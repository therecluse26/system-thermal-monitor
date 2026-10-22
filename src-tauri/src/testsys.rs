//! A fake /sys tree for tests.

use std::fs;
use std::os::unix::fs::symlink;
use std::path::PathBuf;

use crate::sensors::SysPaths;

/// Builds a fake /sys tree. Each `write` path is relative to the tree root.
pub struct FakeSys {
    root: tempfile::TempDir,
}

impl FakeSys {
    pub fn new() -> Self {
        let fake = Self {
            root: tempfile::tempdir().unwrap(),
        };
        fake.write("class/hwmon/.keep", "");
        fake.write("class/thermal/.keep", "");
        fake.write("proc/.keep", "");
        fake.write("cpuinfo", "processor\t: 0\nmodel name\t: Test CPU 9000\n");
        fake
    }

    pub fn path(&self, rel: &str) -> PathBuf {
        self.root.path().join(rel)
    }

    pub fn write(&self, rel: &str, content: &str) {
        let p = self.path(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }

    /// What the app last wrote to a file, without the trailing newline.
    pub fn read(&self, rel: &str) -> String {
        fs::read_to_string(self.path(rel)).unwrap().trim_end().to_string()
    }

    pub fn link_device(&self, hwmon: &str, device_rel: &str) {
        fs::create_dir_all(self.path(device_rel)).unwrap();
        symlink(self.path(device_rel), self.path(&format!("class/hwmon/{hwmon}/device"))).unwrap();
    }

    pub fn paths(&self) -> SysPaths {
        SysPaths {
            hwmon: self.path("class/hwmon"),
            thermal: self.path("class/thermal"),
            cpuinfo: self.path("cpuinfo"),
            proc: self.path("proc"),
        }
    }
}
