use askalono::Store;
use color_eyre::eyre::{eyre, Result, Context};
use flate2::bufread::GzDecoder;
use serde_json::Value;
use std::{
    env,
    io::{BufReader, Write},
    path::{Path, PathBuf},
};

use tar::Archive;

const SPDX_LICENSE_ARCHIVE: &str =
    "https://github.com/spdx/license-list-data/archive/refs/tags/v3.19.tar.gz";
const LICENSE_ROOT_DIR: &str = "license-list-data-3.19";

fn download_license_archive(
    out_dir: impl AsRef<Path>,
    license_archive: impl AsRef<Path>,
) -> Result<()> {
    let client = reqwest::blocking::Client::new();
    let response = client.get(SPDX_LICENSE_ARCHIVE).send()?;
    let tmp_license_archive = out_dir.as_ref().join("license-archive.tar.gz.part");
    _ = std::fs::remove_file(tmp_license_archive.as_path());
    let mut file = std::fs::File::create(tmp_license_archive.as_path())?;
    file.write_all(&response.bytes()?)?;
    file.flush()?;
    drop(file);
    std::fs::rename(tmp_license_archive.as_path(), license_archive.as_ref())?;
    Ok(())
}

fn read_license_archive(
    license_archive: impl AsRef<Path>,
    store: &mut Store,
) -> Result<()> {
    let license_archive = std::fs::File::open(license_archive.as_ref())?;
    let reader = BufReader::new(license_archive);
    let decoder = GzDecoder::new(reader);
    let mut tar = Archive::new(decoder);
    let mut entries = tar.entries().context("Failed to read archive entries")?;

    let json_details_dir = [LICENSE_ROOT_DIR, "json", "details"]
        .iter()
        .collect::<PathBuf>();
    let json_exceptions_dir = [LICENSE_ROOT_DIR, "json", "exceptions"]
        .iter()
        .collect::<PathBuf>();

    while let Some(Ok(entry)) = entries.next() {
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let entry_path = entry.path()?.to_path_buf();
        if entry_path.starts_with(json_details_dir.as_path())
            || entry_path.starts_with(json_exceptions_dir.as_path())
        {
            let reader = std::io::BufReader::new(entry);
            let data: Value = serde_json::from_reader(reader).with_context(|| {
                eyre!(
                    "Failed to deserialize data from file '{}' into json",
                    entry_path.display()
                )
            })?;
            let is_deprecated = data["isDeprecatedLicenseId"].as_bool().expect("missing isDeprecatedLicenseId");
            if is_deprecated {
                continue
            }
            let id = data["licenseId"]
                .as_str()
                .or_else(|| data["licenseExceptionId"].as_str())
                .expect("missing license id");
            let text = data["licenseText"]
                .as_str()
                .or_else(|| data["licenseExceptionText"].as_str())
                .expect("missing license text");
            store.add_license(id.into(), text.into());
        }
    }
    Ok(())
}

fn main() -> Result<()> {
    let out_dir = env::var_os("OUT_DIR").unwrap();
    let license_archive = Path::new(&out_dir).join("license-archive.tar.gz");
    let license_cache = Path::new(&out_dir).join("license-cache.bin.gz");
    if !license_archive.exists() {
        let mut download_attempts = 3;
        while download_attempts > 0 {
            match download_license_archive(&out_dir, &license_archive) {
                Ok(_) => {
                    break;
                }
                Err(_) if download_attempts > 0 => {
                    download_attempts -= 1;
                }
                Err(err) => return Err(err),
            }
        }
        _ = std::fs::remove_file(license_cache.as_path());
    }
    if !license_cache.is_file() {
        let mut store = Store::new();
        read_license_archive(license_archive, &mut store)?;
        let mut cache = std::fs::File::create(license_cache)?;
        store.to_cache(&mut cache).expect("Failed to serialize store");
        cache.sync_all()?;
    }

    Ok(())
}
