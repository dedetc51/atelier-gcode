use crate::gcode::{self, Layer, Metadata, Metrics, SETTINGS};
use anyhow::{Context, Result, bail, ensure};
use md5::{Digest, Md5};
use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::{
    io::{Cursor, Read},
    sync::LazyLock,
};
use zip::ZipArchive;

pub const MAX_BYTES: usize = 64 * 1024 * 1024;
type Archive<'a> = ZipArchive<Cursor<&'a [u8]>>;
static PLATE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^Metadata/plate_[1-9][0-9]{0,3}\.gcode$").unwrap());

#[derive(Serialize)]
struct Setting {
    key: &'static str,
    label: &'static str,
    value: String,
}

#[derive(Serialize)]
pub struct Report {
    filename: String,
    member: Option<String>,
    digest: String,
    printer: String,
    nozzle: String,
    slicer: String,
    duration_s: Option<u64>,
    objects: Vec<f64>,
    filaments: Vec<usize>,
    filament_profile: String,
    settings: Vec<Setting>,
    layers: Vec<Layer>,
    totals: Metrics,
    print_options: Option<PrintPlate>,
    print_error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PlateJson {
    bed_type: String,
    filament_ids: Vec<usize>,
    nozzle_diameter: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct Filament {
    pub index: usize,
    pub material: String,
    pub color: String,
}

#[derive(Debug, Serialize)]
pub struct PrintPlate {
    pub member: String,
    pub nozzle: f64,
    pub bed_type: String,
    pub filaments: Vec<Filament>,
    #[serde(skip)]
    pub project_filaments: usize,
    #[serde(skip)]
    pub md5: String,
}

fn read_member(archive: &mut Archive<'_>, name: &str, limit: usize) -> Result<Vec<u8>> {
    let mut entry = archive
        .by_name(name)
        .context("Fichier interne absent de l’archive.")?;
    ensure!(
        entry.size() <= limit as u64,
        "L’archive décompressée dépasse la taille prise en charge."
    );
    let mut bytes = Vec::with_capacity(entry.size() as usize);
    (&mut entry)
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= limit,
        "L’archive décompressée dépasse la taille prise en charge."
    );
    Ok(bytes)
}

fn open_archive(bytes: &[u8]) -> Result<Archive<'_>> {
    ZipArchive::new(Cursor::new(bytes)).context("Cette archive 3MF ne peut pas être lue.")
}

fn gcode_text(bytes: &[u8]) -> Result<&str> {
    let text = std::str::from_utf8(bytes).context("Le G-code doit être encodé en UTF-8.")?;
    Ok(text.trim_start_matches('\u{feff}'))
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn print_plate(
    archive: &mut Archive<'_>,
    member: &str,
    bytes: &[u8],
    metadata: &Metadata,
) -> Result<PrintPlate> {
    ensure!(
        PLATE.is_match(member),
        "Sélectionne un export de plateau tranché par Bambu Studio."
    );
    ensure!(
        !metadata.slicer.is_empty(),
        "L’archive doit provenir de Bambu Studio."
    );
    let config = &metadata.config;
    ensure!(
        config
            .get("printer_model")
            .is_some_and(|value| value == "Bambu Lab P2S"),
        "Ce plateau doit être tranché avec le profil Bambu Lab P2S."
    );
    let nozzle = config
        .get("nozzle_diameter")
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value > 0.)
        .context("Diamètre de buse absent du G-code.")?;
    let json_name = member.replace(".gcode", ".json");
    let plate: PlateJson = serde_json::from_slice(&read_member(archive, &json_name, 1024 * 1024)?)
        .context("Les métadonnées du plateau sont illisibles.")?;
    ensure!(
        plate
            .nozzle_diameter
            .is_none_or(|diameter| (diameter - nozzle).abs() < 0.01),
        "Les métadonnées et le G-code indiquent des buses différentes."
    );
    ensure!(
        !plate.bed_type.is_empty()
            && plate.bed_type.len() <= 40
            && plate
                .bed_type
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_'),
        "Le type de plateau est invalide."
    );
    let types: Vec<_> = config
        .get("filament_type")
        .map(|value| value.split(';').collect())
        .context("Les types de filament du projet sont absents.")?;
    let colors: Vec<_> = config
        .get("filament_colour")
        .map(|value| value.split(';').collect())
        .unwrap_or_default();
    let mut indices = plate.filament_ids;
    indices.sort_unstable();
    indices.dedup();
    ensure!(
        !indices.is_empty() && types.len() <= 64 && indices.iter().all(|&id| id < types.len()),
        "Les filaments utilisés ne correspondent pas au profil du projet."
    );
    let mut header_indices = metadata.filaments.clone();
    header_indices.sort_unstable();
    header_indices.dedup();
    ensure!(
        header_indices.is_empty()
            || header_indices == indices.iter().map(|id| id + 1).collect::<Vec<_>>(),
        "Les filaments déclarés par le plateau et le G-code diffèrent."
    );
    let mut filaments = Vec::new();
    for index in indices {
        let material = types[index].trim();
        ensure!(
            !material.is_empty() && material.len() <= 40,
            "Type de filament invalide."
        );
        filaments.push(Filament {
            index,
            material: material.into(),
            color: colors.get(index).copied().unwrap_or_default().into(),
        });
    }
    let md5 = format!("{:x}", Md5::digest(bytes));
    let md5_member = format!("{member}.md5");
    if archive.file_names().any(|name| name == md5_member) {
        let declared = read_member(archive, &md5_member, 128)?;
        ensure!(
            std::str::from_utf8(&declared)?
                .trim()
                .eq_ignore_ascii_case(&md5),
            "La somme de contrôle du G-code diffère de celle de l’archive."
        );
    }
    Ok(PrintPlate {
        member: member.into(),
        nozzle,
        bed_type: plate.bed_type,
        filaments,
        project_filaments: types.len(),
        md5,
    })
}

fn report(
    filename: &str,
    member: Option<&str>,
    bytes: &[u8],
    mut archive: Option<&mut Archive<'_>>,
) -> Result<Report> {
    let gcode = gcode_text(bytes)?;
    let metadata = Metadata::read(gcode);
    let (layers, totals) = gcode::analyze(gcode)?;
    let (print_options, print_error) = match (member, archive.as_mut()) {
        (Some(member), Some(archive)) => match print_plate(archive, member, bytes, &metadata) {
            Ok(plate) => (Some(plate), None),
            Err(error) => (None, Some(error.to_string())),
        },
        _ => (None, None),
    };
    let config = &metadata.config;
    let setting = |key: &str| config.get(key).cloned().unwrap_or_default();
    Ok(Report {
        filename: filename.into(),
        member: member.map(str::to_owned),
        digest: sha256(bytes),
        printer: setting("printer_model"),
        nozzle: setting("nozzle_diameter"),
        slicer: metadata.slicer,
        duration_s: metadata.duration_s,
        objects: metadata.objects,
        filaments: metadata.filaments,
        filament_profile: setting("filament_settings_id"),
        settings: SETTINGS
            .iter()
            .filter_map(|&(key, label)| {
                Some(Setting {
                    key,
                    label,
                    value: config.get(key)?.clone(),
                })
            })
            .collect(),
        layers,
        totals,
        print_options,
        print_error,
    })
}

pub fn inspect(filename: &str, bytes: &[u8]) -> Result<Vec<Report>> {
    ensure!(
        !bytes.is_empty() && bytes.len() <= MAX_BYTES,
        "Choisis un export de 1 octet à 64 Mio."
    );
    let extension = filename.to_ascii_lowercase();
    if extension.ends_with(".gcode") {
        return Ok(vec![report(filename, None, bytes, None)?]);
    }
    ensure!(
        extension.ends_with(".3mf"),
        "Choisis un export .gcode ou .gcode.3mf."
    );
    let mut archive = open_archive(bytes)?;
    let mut names: Vec<_> = archive
        .file_names()
        .filter(|name| name.to_ascii_lowercase().ends_with(".gcode"))
        .map(str::to_owned)
        .collect();
    if names.is_empty() {
        bail!("Cette archive ne contient aucun plateau tranché.");
    }
    names.sort();
    let mut remaining = MAX_BYTES;
    let mut reports = Vec::new();
    for name in names {
        let gcode = read_member(&mut archive, &name, remaining)?;
        remaining -= gcode.len();
        reports.push(report(filename, Some(&name), &gcode, Some(&mut archive))?);
    }
    Ok(reports)
}

pub fn prepare(bytes: &[u8], member: &str, expected_digest: &str) -> Result<PrintPlate> {
    ensure!(
        PLATE.is_match(member),
        "Le plateau sélectionné est invalide."
    );
    let mut archive = open_archive(bytes)?;
    let gcode = read_member(&mut archive, member, MAX_BYTES)?;
    ensure!(
        sha256(&gcode) == expected_digest,
        "Le G-code diffère du plateau analysé. Réimporte le fichier."
    );
    let metadata = Metadata::read(gcode_text(&gcode)?);
    print_plate(&mut archive, member, &gcode, &metadata)
}
