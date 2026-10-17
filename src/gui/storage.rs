use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use tidalrs::{AudioQuality, Track};
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Quality {
    #[default]
    High,
    Lossless,
    Max,
}
impl Quality {
    pub fn index(self) -> u32 {
        match self {
            Self::High => 0,
            Self::Lossless => 1,
            Self::Max => 2,
        }
    }
    pub fn from_index(i: u32) -> Self {
        match i {
            1 => Self::Lossless,
            2 => Self::Max,
            _ => Self::High,
        }
    }
    pub fn audio(self) -> AudioQuality {
        match self {
            Self::High => AudioQuality::High,
            Self::Lossless => AudioQuality::Lossless,
            Self::Max => AudioQuality::HiResLossless,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub volume: f64,
    pub quality: Quality,
    pub visualization: u32,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            volume: 35.0,
            quality: Quality::High,
            visualization: 0,
        }
    }
}
impl Preferences {
    pub fn load() -> Self {
        let mut value = crate::online::state_directory()
            .ok()
            .and_then(|d| fs::read(d.join("preferences.json")).ok())
            .and_then(|v| serde_json::from_slice::<Self>(&v).ok())
            .unwrap_or_default();
        value.volume = if value.volume.is_finite() {
            value.volume.clamp(0.0, 100.0)
        } else {
            35.0
        };
        value
    }
    pub fn save(&self) -> Result<(), Box<dyn std::error::Error>> {
        atomic(
            &crate::online::state_directory()?.join("preferences.json"),
            self,
        )
    }
}
pub fn atomic(path: &Path, value: &impl Serialize) -> Result<(), Box<dyn std::error::Error>> {
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    let bytes = serde_json::to_vec(value)?;
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(&temp)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    fs::rename(temp, path)?;
    Ok(())
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Played {
    pub track: Track,
    pub played_at: u64,
}
pub struct History {
    path: PathBuf,
    pub entries: Vec<Played>,
}
impl History {
    pub fn load(user: Option<u64>) -> Self {
        let path = crate::online::state_directory()
            .unwrap_or_default()
            .join(format!("history-{}.json", user.unwrap_or(0)));
        let entries = fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Self { path, entries }
    }
    pub fn record(&mut self, track: &Track) -> Result<(), Box<dyn std::error::Error>> {
        self.entries.retain(|e| e.track.id != track.id);
        self.entries.insert(
            0,
            Played {
                track: track.clone(),
                played_at: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
            },
        );
        self.entries.truncate(200);
        atomic(&self.path, &self.entries)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preference_defaults_and_quality_mapping() {
        let p: Preferences = serde_json::from_str("{}").unwrap();
        assert_eq!(p.volume, 35.0);
        for i in 0..3 {
            assert_eq!(Quality::from_index(i).index(), i);
        }
        assert_eq!(Quality::Max.audio(), AudioQuality::HiResLossless);
    }
    #[test]
    fn preference_roundtrip_is_private() {
        let dir =
            std::env::temp_dir().join(format!("tidal-preferences-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("prefs.json");
        let p = Preferences {
            volume: 22.0,
            quality: Quality::Max,
            visualization: 2,
        };
        atomic(&path, &p).unwrap();
        let copy: Preferences = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(copy.volume, 22.0);
        assert_eq!(copy.quality, Quality::Max);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        fs::remove_dir_all(dir).unwrap();
    }
}
