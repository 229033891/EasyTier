use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use tracing_subscriber::fmt::MakeWriter;

pub(crate) const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;
pub(crate) const MAX_LOG_FILES: usize = 3;
pub(crate) const MAX_LOG_READ_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Clone)]
pub(crate) struct DiagnosticMakeWriter {
    inner: Arc<Mutex<RotatingLog>>,
}

impl DiagnosticMakeWriter {
    pub(crate) fn new(directory: &Path) -> io::Result<Self> {
        Ok(Self {
            inner: Arc::new(Mutex::new(RotatingLog::open(directory)?)),
        })
    }

    pub(crate) fn set_directory(&self, directory: &Path) -> io::Result<()> {
        self.lock()?.set_directory(directory)
    }

    pub(crate) fn clear(&self) -> io::Result<()> {
        self.lock()?.clear()
    }

    pub(crate) fn flush(&self) -> io::Result<()> {
        self.lock()?.flush()
    }

    pub(crate) fn list_files(&self) -> io::Result<Vec<DiagnosticLogFileInfo>> {
        self.lock()?.list_files()
    }

    pub(crate) fn read_file(&self, file_name: &str, max_bytes: u64) -> io::Result<String> {
        self.lock()?.read_file(file_name, max_bytes)
    }

    fn lock(&self) -> io::Result<std::sync::MutexGuard<'_, RotatingLog>> {
        self.inner
            .lock()
            .map_err(|_| io::Error::other("diagnostic log lock poisoned"))
    }
}

#[derive(Clone, Debug)]
pub(crate) struct DiagnosticLogFileInfo {
    pub file_name: String,
    pub size_bytes: u64,
    pub modified_ms: i64,
    pub active: bool,
}

impl<'a> MakeWriter<'a> for DiagnosticMakeWriter {
    type Writer = BufferedEventWriter;

    fn make_writer(&'a self) -> Self::Writer {
        BufferedEventWriter {
            target: self.clone(),
            buffer: Vec::new(),
        }
    }
}

pub(crate) struct BufferedEventWriter {
    target: DiagnosticMakeWriter,
    buffer: Vec<u8>,
}

impl BufferedEventWriter {
    fn commit(&mut self) -> io::Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        let buffer = std::mem::take(&mut self.buffer);
        self.target.lock()?.write_event(&buffer)
    }
}

impl Write for BufferedEventWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.buffer.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.commit()
    }
}

impl Drop for BufferedEventWriter {
    fn drop(&mut self) {
        let _ = self.commit();
    }
}

struct RotatingLog {
    directory: PathBuf,
    active: Option<File>,
    active_bytes: u64,
}

impl RotatingLog {
    fn open(directory: &Path) -> io::Result<Self> {
        fs::create_dir_all(directory)?;
        let mut log = Self {
            directory: directory.to_owned(),
            active: None,
            active_bytes: 0,
        };
        log.truncate_oversized_files()?;
        log.open_active()?;
        Ok(log)
    }

    fn set_directory(&mut self, directory: &Path) -> io::Result<()> {
        if self.directory == directory && self.active.is_some() {
            return Ok(());
        }
        self.flush()?;
        self.active = None;
        self.directory = directory.to_owned();
        fs::create_dir_all(directory)?;
        self.truncate_oversized_files()?;
        self.open_active()
    }

    fn active_path(&self) -> PathBuf {
        self.directory.join("easytier.log")
    }

    fn rotated_path(&self, index: usize) -> PathBuf {
        self.directory.join(format!("easytier.{index}.log"))
    }

    fn truncate_oversized_files(&self) -> io::Result<()> {
        let paths = std::iter::once(self.active_path())
            .chain((1..MAX_LOG_FILES).map(|index| self.rotated_path(index)));
        for path in paths {
            if path
                .metadata()
                .is_ok_and(|metadata| metadata.len() > MAX_LOG_BYTES)
            {
                OpenOptions::new()
                    .write(true)
                    .open(path)?
                    .set_len(MAX_LOG_BYTES)?;
            }
        }
        Ok(())
    }

    fn open_active(&mut self) -> io::Result<()> {
        let path = self.active_path();
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        self.active_bytes = file.metadata()?.len();
        self.active = Some(file);
        if self.active_bytes >= MAX_LOG_BYTES {
            self.rotate()?;
        }
        Ok(())
    }

    fn write_event(&mut self, event: &[u8]) -> io::Result<()> {
        if event.is_empty() {
            return Ok(());
        }
        if self.active_bytes > 0
            && self.active_bytes.saturating_add(event.len() as u64) > MAX_LOG_BYTES
        {
            self.rotate()?;
        }
        let remaining = MAX_LOG_BYTES.saturating_sub(self.active_bytes) as usize;
        let event = &event[..event.len().min(remaining)];
        if let Some(active) = self.active.as_mut() {
            active.write_all(event)?;
            self.active_bytes += event.len() as u64;
        }
        Ok(())
    }

    fn rotate(&mut self) -> io::Result<()> {
        self.flush()?;
        self.active = None;

        let oldest = self.rotated_path(MAX_LOG_FILES - 1);
        if oldest.exists() {
            fs::remove_file(oldest)?;
        }
        for index in (1..MAX_LOG_FILES - 1).rev() {
            let source = self.rotated_path(index);
            if source.exists() {
                fs::rename(source, self.rotated_path(index + 1))?;
            }
        }
        let active = self.active_path();
        if active.exists() {
            fs::rename(active, self.rotated_path(1))?;
        }
        self.active_bytes = 0;
        self.active = Some(
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.active_path())?,
        );
        Ok(())
    }

    fn clear(&mut self) -> io::Result<()> {
        self.flush()?;
        self.active = None;
        for index in 1..MAX_LOG_FILES {
            let path = self.rotated_path(index);
            if path.exists() {
                fs::remove_file(path)?;
            }
        }
        let active = self.active_path();
        if active.exists() {
            fs::remove_file(&active)?;
        }
        self.active_bytes = 0;
        self.active = Some(OpenOptions::new().create(true).append(true).open(active)?);
        Ok(())
    }

    fn flush(&mut self) -> io::Result<()> {
        match self.active.as_mut() {
            Some(active) => active.flush(),
            None => Ok(()),
        }
    }

    fn list_files(&self) -> io::Result<Vec<DiagnosticLogFileInfo>> {
        let mut files = Vec::new();
        let active = self.active_path();
        if active.exists()
            && let Ok(info) = file_info(&active, true)
        {
            files.push(info);
        }
        for index in 1..MAX_LOG_FILES {
            let path = self.rotated_path(index);
            if path.exists() && let Ok(info) = file_info(&path, false) {
                files.push(info);
            }
        }
        Ok(files)
    }

    fn read_file(&self, file_name: &str, max_bytes: u64) -> io::Result<String> {
        let max_bytes = max_bytes.min(MAX_LOG_READ_BYTES);
        let name = Path::new(file_name)
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid log file name"))?;
        let path = if name == "easytier.log" {
            self.active_path()
        } else if let Some(index) = name
            .strip_prefix("easytier.")
            .and_then(|rest| rest.strip_suffix(".log"))
            .and_then(|index| index.parse::<usize>().ok())
        {
            if index == 0 || index >= MAX_LOG_FILES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "invalid log file name",
                ));
            }
            self.rotated_path(index)
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid log file name",
            ));
        };
        read_file_tail(&path, max_bytes)
    }
}

fn file_info(path: &Path, active: bool) -> io::Result<DiagnosticLogFileInfo> {
    let meta = path.metadata()?;
    let modified_ms = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0);
    Ok(DiagnosticLogFileInfo {
        file_name: path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("easytier.log")
            .to_string(),
        size_bytes: meta.len(),
        modified_ms,
        active,
    })
}

fn read_file_tail(path: &Path, max_bytes: u64) -> io::Result<String> {
    use std::io::{Read, Seek, SeekFrom};

    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    if len > max_bytes {
        file.seek(SeekFrom::End(-(max_bytes as i64)))?;
    }
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;
    let content = String::from_utf8_lossy(&buf);
    if len > max_bytes
        && let Some(rest) = content.split_once('\n').map(|(_, rest)| rest)
    {
        return Ok(rest.to_string());
    }
    Ok(content.into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!("easytier-ios-{name}-{unique}"));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn rotates_without_exceeding_file_limit() {
        let directory = TempDir::new("rotation");
        let mut log = RotatingLog::open(&directory.0).unwrap();
        let event = vec![b'x'; (MAX_LOG_BYTES / 2 + 1) as usize];

        for _ in 0..6 {
            log.write_event(&event).unwrap();
        }
        log.flush().unwrap();

        let files = fs::read_dir(&directory.0)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(files.len(), MAX_LOG_FILES);
        assert!(
            files
                .iter()
                .all(|entry| entry.metadata().unwrap().len() <= MAX_LOG_BYTES)
        );
    }

    #[test]
    fn clear_removes_rotated_content_and_keeps_active_file_writable() {
        let directory = TempDir::new("clear");
        let mut log = RotatingLog::open(&directory.0).unwrap();
        let event = vec![b'x'; (MAX_LOG_BYTES / 2 + 1) as usize];
        log.write_event(&event).unwrap();
        log.write_event(&event).unwrap();

        log.clear().unwrap();
        log.write_event(b"after clear\n").unwrap();
        log.flush().unwrap();

        assert_eq!(fs::read(log.active_path()).unwrap(), b"after clear\n");
        assert!(!log.rotated_path(1).exists());
    }

    #[test]
    fn opening_truncates_oversized_known_files() {
        let directory = TempDir::new("oversized");
        for name in ["easytier.log", "easytier.1.log"] {
            let file = File::create(directory.0.join(name)).unwrap();
            file.set_len(MAX_LOG_BYTES + 1).unwrap();
        }

        let log = RotatingLog::open(&directory.0).unwrap();

        for index in 1..MAX_LOG_FILES {
            let path = log.rotated_path(index);
            if path.exists() {
                assert!(path.metadata().unwrap().len() <= MAX_LOG_BYTES);
            }
        }
        assert!(log.active_path().metadata().unwrap().len() <= MAX_LOG_BYTES);
    }
}
