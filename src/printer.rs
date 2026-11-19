use crate::archive::PrintPlate;
use anyhow::{Context, Result, bail, ensure};
use curl::easy::{Easy, SslVersion};
use native_tls::TlsConnector;
use rumqttc::{Client, Connection, Event, Incoming, MqttOptions, QoS, RecvTimeoutError, Transport};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    io::Read,
    net::Ipv4Addr,
    time::{Duration, Instant},
};
use uuid::Uuid;

const EXTERNAL: u16 = 254;
const TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Deserialize)]
pub struct Credentials {
    ip: Ipv4Addr,
    serial: String,
    access_code: String,
}

impl Credentials {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.ip.is_private() || self.ip.is_link_local(),
            "Indique l’adresse IPv4 locale affichée sur la P2S."
        );
        ensure!(
            (8..=32).contains(&self.serial.len())
                && self.serial.starts_with("22E")
                && self.serial.bytes().all(|byte| byte.is_ascii_alphanumeric()),
            "Saisis le numéro de série de la P2S (préfixe 22E)."
        );
        ensure!(
            self.access_code.len() == 8
                && self
                    .access_code
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric()),
            "Saisis le code d’accès LAN à huit caractères affiché sur la P2S."
        );
        Ok(())
    }
}

#[derive(Serialize)]
pub struct Tray {
    id: u16,
    label: String,
    material: String,
    color: String,
}

#[derive(Serialize)]
pub struct PrinterStatus {
    state: String,
    nozzle: Option<f64>,
    trays: Vec<Tray>,
}

#[derive(Serialize)]
pub struct Dispatch {
    state: &'static str,
    job: String,
}

struct PrinterSession {
    client: Client,
    connection: Connection,
    request: String,
    report: String,
}

impl PrinterSession {
    fn connect(credentials: &Credentials) -> Result<Self> {
        // The P2S presents a device certificate that is not in the system trust store.
        // Pair only on a trusted LAN using the address and serial from the device.
        let connector = TlsConnector::builder()
            .danger_accept_invalid_certs(true)
            .danger_accept_invalid_hostnames(true)
            .build()?;
        let mut options = MqttOptions::new(
            format!("atelier-{}", &Uuid::new_v4().simple().to_string()[..12]),
            credentials.ip.to_string(),
            8883,
        );
        options.set_credentials("bblp", &credentials.access_code);
        options.set_keep_alive(Duration::from_secs(15));
        options.set_max_packet_size(1024 * 1024, 32 * 1024);
        options.set_transport(Transport::tls_with_config(connector.into()));
        let (client, connection) = Client::new(options, 8);
        let mut session = Self {
            client,
            connection,
            request: format!("device/{}/request", credentials.serial),
            report: format!("device/{}/report", credentials.serial),
        };
        session.client.subscribe(&session.report, QoS::AtMostOnce)?;
        let deadline = Instant::now() + TIMEOUT;
        while let Some(event) = session.next(deadline)? {
            if let Event::Incoming(Incoming::SubAck(ack)) = event {
                ensure!(
                    ack.return_codes
                        .contains(&rumqttc::SubscribeReasonCode::Success(QoS::AtMostOnce)),
                    "La P2S a refusé l’accès aux rapports LAN."
                );
                return Ok(session);
            }
        }
        bail!("La P2S n’a pas répondu à la connexion MQTT.");
    }

    fn next(&mut self, deadline: Instant) -> Result<Option<Event>> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(None);
        }
        match self.connection.recv_timeout(remaining) {
            Ok(event) => Ok(Some(
                event.context("La connexion MQTT à la P2S a été interrompue.")?,
            )),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(_) => bail!("La connexion MQTT à la P2S a été fermée."),
        }
    }

    fn publish(&self, command: Value) -> Result<()> {
        self.client.publish(
            &self.request,
            QoS::AtMostOnce,
            false,
            serde_json::to_vec(&command)?,
        )?;
        Ok(())
    }

    fn next_report(&mut self, deadline: Instant) -> Result<Option<Value>> {
        while let Some(event) = self.next(deadline)? {
            if let Event::Incoming(Incoming::Publish(message)) = event
                && message.topic == self.report
                && !message.retain
                && let Ok(payload) = serde_json::from_slice::<Value>(&message.payload)
                && let Some(report) = payload.get("print")
            {
                return Ok(Some(report.clone()));
            }
        }
        Ok(None)
    }

    fn status(&mut self) -> Result<PrinterStatus> {
        self.publish(json!({"pushing":{"sequence_id":"0","command":"pushall"}}))?;
        let deadline = Instant::now() + TIMEOUT;
        let mut fallback = None;
        while let Some(report) = self.next_report(deadline)? {
            if let Some(state) = parse_status(&report) {
                if report["ams"]["ams"].is_array() {
                    return Ok(state);
                }
                fallback = Some(state);
            }
        }
        if let Some(state) = fallback {
            return Ok(state);
        }
        bail!(
            "La P2S n’a pas renvoyé son état. Vérifie le numéro de série, le code d’accès et le mode LAN."
        );
    }

    fn wait_for_print(&mut self, id: &str, job: &str) -> Result<&'static str> {
        let deadline = Instant::now() + Duration::from_secs(18);
        let mut accepted = false;
        // An interruption after publish is ambiguous. Never re-send the command here.
        while let Ok(Some(report)) = self.next_report(deadline) {
            if report["command"] == "project_file" && matches_id(&report["sequence_id"], id) {
                let result = report["result"].as_str().unwrap_or_default();
                if result.eq_ignore_ascii_case("FAIL")
                    || result.eq_ignore_ascii_case("FAILED")
                    || result.eq_ignore_ascii_case("ERROR")
                {
                    bail!(
                        "La P2S a refusé le lancement. Consulte son écran et vérifie le mode développeur."
                    );
                }
                accepted =
                    result.eq_ignore_ascii_case("SUCCESS") || report["result"].as_u64() == Some(0);
            }
            let state = report["gcode_state"].as_str().unwrap_or("");
            let same_job = ["task_id", "subtask_id"]
                .iter()
                .any(|key| matches_id(&report[*key], id))
                || ["subtask_name", "gcode_file"]
                    .iter()
                    .filter_map(|key| report[*key].as_str())
                    .any(|value| {
                        value.rsplit('/').next().is_some_and(|name| {
                            name == job || name.strip_suffix(".gcode.3mf") == Some(job)
                        })
                    });
            if same_job && matches!(state, "PREPARE" | "SLICING" | "RUNNING") {
                return Ok("started");
            }
        }
        Ok(if accepted { "accepted" } else { "unconfirmed" })
    }
}

fn matches_id(value: &Value, id: &str) -> bool {
    value.as_str() == Some(id)
        || value
            .as_u64()
            .is_some_and(|number| number.to_string() == id)
}

fn index(value: &Value) -> Option<u16> {
    value
        .as_u64()
        .and_then(|number| number.try_into().ok())
        .or_else(|| value.as_str()?.parse().ok())
}

fn tray_color(tray: &Value) -> String {
    let value = tray["tray_color"]
        .as_str()
        .unwrap_or_default()
        .trim_start_matches('#');
    if matches!(value.len(), 6 | 8) && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        format!("#{}", &value[..6])
    } else {
        String::new()
    }
}

fn parse_status(report: &Value) -> Option<PrinterStatus> {
    let state = report["gcode_state"].as_str()?.to_ascii_uppercase();
    let nozzle = report["nozzle_diameter"]
        .as_f64()
        .or_else(|| report["nozzle_diameter"].as_str()?.parse().ok())
        .filter(|diameter| diameter.is_finite() && *diameter > 0.);
    let mut trays = Vec::new();
    let loaded = report["ams"]["tray_exist_bits"]
        .as_str()
        .and_then(|bits| u64::from_str_radix(bits, 16).ok());
    if let Some(units) = report["ams"]["ams"].as_array() {
        for unit in units {
            let Some(ams_id) = index(&unit["id"]) else {
                continue;
            };
            let Some(slots) = unit["tray"].as_array() else {
                continue;
            };
            for slot in slots {
                let Some(slot_id) = index(&slot["id"]) else {
                    continue;
                };
                let material = slot["tray_type"].as_str().unwrap_or_default().trim();
                if material.is_empty() {
                    continue;
                }
                let (id, label) = match (ams_id, slot_id) {
                    (0..=3, 0..=3) => (
                        ams_id * 4 + slot_id,
                        format!("AMS {} · emplacement {}", ams_id + 1, slot_id + 1),
                    ),
                    (128..=253, 0) => (ams_id, format!("AMS HT {}", ams_id - 127)),
                    _ => continue,
                };
                if id <= 15 && loaded.is_some_and(|bits| bits & (1 << id) == 0) {
                    continue;
                }
                trays.push(Tray {
                    id,
                    label,
                    material: material.into(),
                    color: tray_color(slot),
                });
            }
        }
    }
    trays.sort_by_key(|tray| tray.id);
    trays.dedup_by_key(|tray| tray.id);
    let virtual_tray = report["vir_slot"]
        .as_array()
        .and_then(|slots| {
            slots
                .iter()
                .find(|slot| index(&slot["id"]) == Some(EXTERNAL))
        })
        .unwrap_or(&report["vt_tray"]);
    trays.push(Tray {
        id: EXTERNAL,
        label: "Bobine externe".into(),
        material: virtual_tray["tray_type"]
            .as_str()
            .unwrap_or_default()
            .into(),
        color: tray_color(virtual_tray),
    });
    Some(PrinterStatus {
        state,
        nozzle,
        trays,
    })
}

pub fn status(credentials: &Credentials) -> Result<PrinterStatus> {
    PrinterSession::connect(credentials)?.status()
}

fn check(status: &PrinterStatus, plate: &PrintPlate, selected: &[u16]) -> Result<()> {
    ensure!(
        matches!(status.state.as_str(), "IDLE" | "FINISH"),
        "La P2S signale l’état {}. Attends qu’elle soit disponible.",
        status.state
    );
    ensure!(
        !status
            .nozzle
            .is_some_and(|actual| (actual - plate.nozzle).abs() >= 0.01),
        "La buse déclarée par la P2S diffère de celle du tranchage ({} mm).",
        plate.nozzle
    );
    ensure!(
        selected.len() == plate.filaments.len(),
        "Associe chaque filament à un emplacement."
    );
    ensure!(
        selected.len() == 1 || !selected.contains(&EXTERNAL),
        "Pour ce plateau multifilament, choisis un emplacement AMS pour chaque filament."
    );
    ensure!(
        selected.iter().copied().collect::<HashSet<_>>().len() == selected.len(),
        "Choisis un emplacement différent pour chaque filament du projet."
    );
    for (filament, selected) in plate.filaments.iter().zip(selected) {
        let tray = status
            .trays
            .iter()
            .find(|tray| tray.id == *selected)
            .with_context(|| {
                format!(
                    "L’emplacement choisi pour le filament {} n’est pas déclaré par la P2S.",
                    filament.index + 1
                )
            })?;
        ensure!(
            tray.material.is_empty() || tray.material.eq_ignore_ascii_case(&filament.material),
            "Le filament {} attend du {}. L’emplacement annonce du {}.",
            filament.index + 1,
            filament.material,
            tray.material
        );
    }
    Ok(())
}

fn upload(credentials: &Credentials, filename: &str, archive: &[u8]) -> Result<()> {
    let mut ftp = Easy::new();
    // The double slash addresses the absolute /cache directory.
    ftp.url(&format!("ftps://{}:990//cache/{filename}", credentials.ip))?;
    ftp.proxy("")?;
    ftp.username("bblp")?;
    ftp.password(&credentials.access_code)?;
    ftp.ssl_verify_peer(false)?;
    ftp.ssl_verify_host(false)?;
    // The P2S data connection shares the control TLS session. TLS 1.2 avoids
    // the delayed session ticket observed on this model with some FTPS stacks.
    ftp.ssl_min_max_version(SslVersion::Tlsv12, SslVersion::Tlsv12)?;
    ftp.ssl_sessionid_cache(true)?;
    ftp.connect_timeout(TIMEOUT)?;
    ftp.timeout(Duration::from_secs(240))?;
    ftp.upload(true)?;
    ftp.in_filesize(archive.len() as u64)?;
    let mut bytes = archive;
    let mut transfer = ftp.transfer();
    transfer.read_function(|buffer| Ok(bytes.read(buffer).unwrap_or(0)))?;
    transfer
        .perform()
        .context("Le transfert FTPS vers la P2S a échoué.")?;
    drop(transfer);
    ensure!(
        ftp.response_code()? / 100 == 2 && bytes.is_empty(),
        "Le transfert FTPS du plateau est incomplet."
    );
    Ok(())
}

fn command(plate: &PrintPlate, selected: &[u16], filename: &str, job: &str, id: &str) -> Value {
    let mut mapping = vec![-1i32; plate.project_filaments];
    let mut mapping2 = vec![json!({"ams_id":255, "slot_id":255}); plate.project_filaments];
    for (filament, &tray) in plate.filaments.iter().zip(selected) {
        mapping[filament.index] = tray.into();
        let (ams_id, slot_id) = match tray {
            EXTERNAL => (254, 254),
            128..=253 => (tray, 0),
            _ => (tray / 4, tray % 4),
        };
        mapping2[filament.index] = json!({"ams_id":ams_id, "slot_id":slot_id});
    }
    json!({"print": {
        "sequence_id": id, "command": "project_file", "param": plate.member,
        "file": filename, "url": format!("ftp:///cache/{filename}"), "md5": plate.md5,
        "subtask_name": job, "project_id": id, "profile_id": "0", "task_id": id, "subtask_id": id,
        "bed_type": plate.bed_type, "bed_leveling": 1, "auto_bed_leveling": 2,
        "flow_cali": 0, "vibration_cali": 0, "layer_inspect": 1, "timelapse": 0, "cfg": "0",
        "extrude_cali_flag": 0, "extrude_cali_manual_mode": 0, "nozzle_offset_cali": 2,
        "use_ams": !selected.contains(&EXTERNAL), "ams_mapping": mapping, "ams_mapping2": mapping2
    }})
}

pub fn start(
    credentials: &Credentials,
    plate: &PrintPlate,
    trays: &[u16],
    archive: &[u8],
) -> Result<Dispatch> {
    check(&status(credentials)?, plate, trays)?;
    let uuid = Uuid::new_v4();
    let job = format!("atelier-{}", &uuid.simple().to_string()[..16]);
    let filename = format!("{job}.gcode.3mf");
    let id = ((uuid.as_u128() & 0x7fff_ffff) + 1).to_string();
    upload(credentials, &filename, archive)?;
    let mut session = PrinterSession::connect(credentials)?;
    check(&session.status()?, plate, trays)?;
    session.publish(command(plate, trays, &filename, &job, &id))?;
    let state = session.wait_for_print(&id, &job)?;
    Ok(Dispatch { state, job })
}
