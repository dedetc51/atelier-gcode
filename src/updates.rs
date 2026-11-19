use anyhow::{Context, Result, bail, ensure};
use curl::easy::Easy;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{fs, process::Command, time::Duration};
use tokio::sync::watch;

const REPO: &str = "dedetc51/atelier-gcode";
const ASSET: &str = "Atelier-Gcode-Setup-x64.exe";
const MAX_SETUP: usize = 64 * 1024 * 1024;
const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(windows)]
pub fn hold_installation() -> Result<Option<fs::File>> {
    use std::os::windows::fs::OpenOptionsExt;
    let exe = std::env::current_exe()?;
    let folder = exe
        .parent()
        .context("Dossier de l’application introuvable.")?;
    if !folder.join("Desinstaller.exe").is_file() {
        return Ok(None);
    }
    // Allow other application instances to read the marker, but prevent its deletion
    // until all of them have exited. Installation of a newer version can proceed.
    Ok(Some(
        fs::OpenOptions::new()
            .read(true)
            .share_mode(1 | 2)
            .open(folder.join("session.lock"))?,
    ))
}

#[derive(Clone, Serialize)]
pub struct Status {
    version: &'static str,
    message: String,
    checking: bool,
}

pub fn start(installed: bool) -> watch::Receiver<Status> {
    let checking = cfg!(windows) && installed;
    let initial = Status {
        version: VERSION,
        message: if checking {
            "Recherche de mises à jour…".into()
        } else {
            String::new()
        },
        checking,
    };
    let (sender, receiver) = watch::channel(initial);
    if checking {
        tokio::task::spawn_blocking(move || {
            let message = run().unwrap_or_else(|error| {
                eprintln!("Mise à jour différée. {error:#}");
                "Mise à jour différée. Nouvel essai à la prochaine ouverture.".into()
            });
            sender.send_replace(Status {
                version: VERSION,
                message,
                checking: false,
            });
        });
    }
    receiver
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    size: usize,
    digest: Option<String>,
    state: String,
}

// Windows version resources use three unsigned 16-bit components and a final zero.
fn version(text: &str) -> Option<[u16; 3]> {
    let parts: Vec<_> = text.split('.').collect();
    let [a, b, c] = parts[..] else { return None };
    let number = |s: &str| {
        if s.is_empty()
            || !s.bytes().all(|b| b.is_ascii_digit())
            || (s.len() > 1 && s.starts_with('0'))
        {
            return None;
        }
        s.parse().ok()
    };
    Some([number(a)?, number(b)?, number(c)?])
}

struct Package<'a> {
    version: &'a str,
    size: usize,
    digest: &'a str,
}

fn newer<'a>(release: &'a Release, current: &str) -> Result<Option<Package<'a>>> {
    if release.draft || release.prerelease {
        return Ok(None);
    }
    let tag = release
        .tag_name
        .strip_prefix('v')
        .context("Tag de version inconnu.")?;
    let latest = version(tag).context("Le tag ne désigne pas une version stable.")?;
    if latest <= version(current).context("Version installée illisible.")? {
        return Ok(None);
    }
    let asset = release
        .assets
        .iter()
        .find(|a| a.name == ASSET && a.state == "uploaded")
        .context("Installateur Windows absent de la publication.")?;
    let digest = asset
        .digest
        .as_deref()
        .and_then(|d| d.strip_prefix("sha256:"))
        .context("Empreinte SHA-256 absente de la publication.")?;
    ensure!(
        digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit()),
        "Empreinte SHA-256 illisible."
    );
    ensure!(
        (1..=MAX_SETUP).contains(&asset.size),
        "Taille de l’installateur inattendue."
    );
    Ok(Some(Package {
        version: tag,
        size: asset.size,
        digest,
    }))
}

fn verified(package: &Package<'_>, bytes: &[u8]) -> Result<()> {
    ensure!(bytes.len() == package.size, "Téléchargement incomplet.");
    let actual = format!("{:x}", Sha256::digest(bytes));
    ensure!(
        actual.eq_ignore_ascii_case(package.digest),
        "Empreinte de l’installateur incorrecte."
    );
    Ok(())
}

fn github_url(url: &str) -> bool {
    [
        "https://api.github.com/",
        "https://github.com/",
        "https://release-assets.githubusercontent.com/",
        "https://objects.githubusercontent.com/",
    ]
    .iter()
    .any(|prefix| url.starts_with(prefix))
}

fn get(mut url: String, limit: usize, timeout: Duration) -> Result<(u32, Vec<u8>)> {
    for _ in 0..5 {
        ensure!(github_url(&url), "Adresse de téléchargement inattendue.");
        // This handle uses normal OS certificate and hostname verification.
        let mut http = Easy::new();
        http.url(&url)?;
        http.useragent(concat!("Atelier-Gcode/", env!("CARGO_PKG_VERSION")))?;
        http.connect_timeout(Duration::from_secs(5))?;
        http.timeout(timeout)?;
        http.max_filesize(limit as u64)?;
        let mut bytes = Vec::new();
        {
            let mut transfer = http.transfer();
            transfer.write_function(|chunk| {
                if chunk.len() > limit - bytes.len() {
                    return Ok(0);
                }
                bytes.extend_from_slice(chunk);
                Ok(chunk.len())
            })?;
            transfer.perform()?;
        }
        let status = http.response_code()?;
        if matches!(status, 301 | 302 | 303 | 307 | 308) {
            url = http
                .redirect_url()?
                .context("Redirection incomplète.")?
                .to_owned();
            continue;
        }
        return Ok((status, bytes));
    }
    bail!("Trop de redirections pendant le téléchargement.")
}

fn run() -> Result<String> {
    let exe = std::env::current_exe()?;
    let folder = exe
        .parent()
        .context("Dossier d’installation introuvable.")?;
    ensure!(
        folder.join("Desinstaller.exe").is_file(),
        "Installation Windows introuvable."
    );
    let lock = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(folder.join("update.lock"))?;
    match lock.try_lock() {
        Ok(()) => {}
        Err(fs::TryLockError::WouldBlock) => {
            return Ok("Une autre fenêtre vérifie les mises à jour.".into());
        }
        Err(fs::TryLockError::Error(err)) => return Err(err.into()),
    }
    let (status, data) = get(
        format!("https://api.github.com/repos/{REPO}/releases/latest"),
        1024 * 1024,
        Duration::from_secs(12),
    )?;
    if status == 404 {
        return Ok("Aucune version publiée sur GitHub.".into());
    }
    ensure!(status == 200, "GitHub a répondu HTTP {status}.");
    let release: Release = serde_json::from_slice(&data)?;
    let Some(package) = newer(&release, VERSION)? else {
        return Ok("À jour.".into());
    };
    let url = format!(
        "https://github.com/{REPO}/releases/download/v{}/{ASSET}",
        package.version
    );
    let (status, bytes) = get(url, package.size, Duration::from_secs(120))?;
    ensure!(status == 200, "Téléchargement refusé (HTTP {status}).");
    verified(&package, &bytes)?;
    let staging = tempfile::Builder::new()
        .prefix("atelier-gcode-")
        .tempdir()?;
    let setup = staging.path().join(ASSET);
    fs::write(&setup, bytes)?;
    // Wait for NSIS so the temporary installer exists for its entire lifetime.
    // The running application and any active printer transfer keep their own executable.
    let outcome = Command::new(setup)
        .args(["/S", "/UPDATE"])
        .status()
        .context("Lancement de l’installateur impossible.")?;
    ensure!(outcome.success(), "L’installateur a renvoyé {outcome}.");
    Ok(format!(
        "Version {} installée. Elle s’ouvrira au prochain lancement depuis le menu Démarrer.",
        package.version
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release() -> Release {
        Release {
            tag_name: "v0.10.0".into(),
            draft: false,
            prerelease: false,
            assets: vec![Asset {
                name: ASSET.into(),
                state: "uploaded".into(),
                size: 3,
                digest: Some(format!("sha256:{:x}", Sha256::digest(b"abc"))),
            }],
        }
    }

    #[test]
    fn only_newer_stable_versions_with_the_expected_installer() {
        let mut r = release();
        assert!(newer(&r, "0.9.0").unwrap().is_some());
        assert!(newer(&r, "0.10.0").unwrap().is_none());
        assert!(newer(&r, "1.0.0").unwrap().is_none());
        r.prerelease = true;
        assert!(newer(&r, "0.9.0").unwrap().is_none());
        r.prerelease = false;
        r.draft = true;
        assert!(newer(&r, "0.9.0").unwrap().is_none());
        r.draft = false;
        r.tag_name = "v0.11.0/other".into();
        assert!(newer(&r, "0.9.0").is_err());
        r.tag_name = "v0.10.0".into();
        r.assets[0].name = "autre.exe".into();
        assert!(newer(&r, "0.9.0").is_err());
    }

    #[test]
    fn verifies_both_size_and_sha_before_installation() {
        let mut r = release();
        let p = newer(&r, "0.9.0").unwrap().unwrap();
        verified(&p, b"abc").unwrap();
        assert!(verified(&p, b"ab").is_err());
        assert!(verified(&p, b"abd").is_err());
        r.assets[0].digest = None;
        assert!(newer(&r, "0.9.0").is_err());
    }

    #[test]
    fn redirects_stay_on_github_over_tls() {
        assert!(github_url(
            "https://release-assets.githubusercontent.com/a?b=1"
        ));
        for url in [
            "http://github.com/file",
            "https://github.com.evil.example/file",
            "https://github.com@other.example/file",
            "file:///tmp/setup",
        ] {
            assert!(!github_url(url));
        }
    }
}
