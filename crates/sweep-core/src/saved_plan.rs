//! Local, reviewable saved plans. The checksum detects accidental modification;
//! it is not a signature and does not make an untrusted plan safe to execute.

use crate::cleaner::CleanPlan;
use crate::model::{CleanAction, Disposition};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::Path;

const PLAN_SCHEMA_VERSION: u32 = 2;
const MAX_PLAN_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedPlan {
    pub plan_schema_version: u32,
    pub plan_id: String,
    pub plan: CleanPlan,
}

impl SavedPlan {
    pub fn new(plan: CleanPlan) -> io::Result<Self> {
        let plan_id = checksum(PLAN_SCHEMA_VERSION, &plan)?;
        let saved = Self {
            plan_schema_version: PLAN_SCHEMA_VERSION,
            plan_id,
            plan,
        };
        saved.validate()?;
        Ok(saved)
    }

    pub fn validate(&self) -> io::Result<()> {
        if self.plan_schema_version != PLAN_SCHEMA_VERSION {
            return Err(io::Error::other("unsupported saved-plan schema version; create and review a new plan with this Sweep version"));
        }
        if checksum(self.plan_schema_version, &self.plan)? != self.plan_id {
            return Err(io::Error::other(
                "saved plan checksum mismatch; create and review a new plan",
            ));
        }
        let mut paths = Vec::new();
        let mut total = 0_u64;
        for item in &self.plan.items {
            if let Disposition::Remove { via } = &item.disposition {
                let CleanAction::RemovePath { path } = &item.finding.action else {
                    return Err(io::Error::other("saved plan removal has no path"));
                };
                if !path.is_absolute() || !matches!(via.as_str(), "trash" | "permanent") {
                    return Err(io::Error::other("invalid saved plan path or deletion mode"));
                }
                let snapshot = item
                    .snapshot
                    .as_ref()
                    .ok_or_else(|| io::Error::other("saved removal has no snapshot"))?;
                if snapshot.bytes != item.finding.bytes {
                    return Err(io::Error::other(
                        "saved plan byte total differs from snapshot",
                    ));
                }
                if paths
                    .iter()
                    .any(|other: &&Path| path.starts_with(other) || other.starts_with(path))
                {
                    return Err(io::Error::other("saved plan has overlapping removal paths"));
                }
                paths.push(path.as_path());
                total = total
                    .checked_add(snapshot.bytes)
                    .ok_or_else(|| io::Error::other("saved plan byte count overflow"))?;
            }
        }
        if total != self.plan.total_bytes {
            return Err(io::Error::other("saved plan total is inconsistent"));
        }
        Ok(())
    }

    /// Create, never overwrite. The plan file must not sit inside a removal target.
    pub fn write_new(&self, path: &Path) -> io::Result<()> {
        self.validate()?;
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let destination = fs::canonicalize(parent)?.join(
            path.file_name()
                .ok_or_else(|| io::Error::other("plan needs a filename"))?,
        );
        if self.plan.items.iter().any(|item| {
            matches!((&item.disposition, &item.finding.action), (Disposition::Remove { .. }, CleanAction::RemovePath { path }) if destination.starts_with(path))
        }) {
            return Err(io::Error::other("plan file cannot be inside a cleanup target"));
        }
        let data = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        if data.len() as u64 > MAX_PLAN_BYTES {
            return Err(io::Error::other("saved plan exceeds size limit"));
        }
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(destination)?;
        file.write_all(&data)?;
        file.write_all(b"\n")?;
        file.sync_all()
    }

    pub fn read(path: &Path) -> io::Result<Self> {
        let file = fs::File::open(path)?;
        let mut data = Vec::new();
        file.take(MAX_PLAN_BYTES + 1).read_to_end(&mut data)?;
        if data.len() as u64 > MAX_PLAN_BYTES {
            return Err(io::Error::other("saved plan exceeds size limit"));
        }
        let saved: Self = serde_json::from_slice(&data).map_err(io::Error::other)?;
        saved.validate()?;
        Ok(saved)
    }
}

fn checksum(version: u32, plan: &CleanPlan) -> io::Result<String> {
    let data = serde_json::to_vec(&(version, plan)).map_err(io::Error::other)?;
    Ok(format!("{:x}", Sha256::digest(data)))
}
