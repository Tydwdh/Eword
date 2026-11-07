use anyhow::{Context, Result};
use std::fs;
use std::io::Write;
use std::path::Path;
use tempfile::NamedTempFile;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

pub fn now_ts() -> i64 {
    OffsetDateTime::now_utc().unix_timestamp()
}

pub fn rfc3339_now() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

pub fn timestamp_slug() -> String {
    let format = time::macros::format_description!("[year][month][day]-[hour][minute][second]");
    OffsetDateTime::now_utc()
        .format(&format)
        .unwrap_or_else(|_| "19700101-000000".to_string())
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)
        .with_context(|| format!("creating directory {}", parent.display()))?;
    let mut tmp = NamedTempFile::new_in(parent)
        .with_context(|| format!("creating temporary file in {}", parent.display()))?;
    tmp.write_all(bytes)?;
    tmp.as_file().sync_all()?;
    match tmp.persist(path) {
        Ok(_) => Ok(()),
        Err(err) => {
            let tempfile::PersistError { error, file } = err;
            let retry = matches!(error.kind(), std::io::ErrorKind::AlreadyExists)
                || matches!(error.raw_os_error(), Some(80) | Some(183));
            // Windows rename based persistence can refuse to replace an existing
            // destination. The export is a full snapshot, so replacing is safe.
            if retry && path.exists() {
                fs::remove_file(path).with_context(|| format!("replacing {}", path.display()))?;
                file.persist(path)
                    .map(|_| ())
                    .map_err(|err| err.error.into())
            } else {
                Err(error.into())
            }
        }
    }
}

pub fn sql_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}
