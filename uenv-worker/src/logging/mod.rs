use std::fs::{File, OpenOptions, create_dir_all};
use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex};

use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::writer::MakeWriter;

#[derive(Clone)]
struct SharedFileWriter {
    file: Arc<Mutex<File>>,
}

impl<'a> MakeWriter<'a> for SharedFileWriter {
    type Writer = SharedGuard;
    fn make_writer(&'a self) -> Self::Writer {
        SharedGuard {
            file: self.file.clone(),
        }
    }
}

struct SharedGuard {
    file: Arc<Mutex<File>>,
}

impl io::Write for SharedGuard {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut f = self.file.lock().expect("log file mutex poisoned");
        io::Write::write(&mut *f, buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut f = self.file.lock().expect("log file mutex poisoned");
        io::Write::flush(&mut *f)
    }
}

pub fn init(level: &str, path: &str) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = Path::new(path).parent() {
        if !parent.as_os_str().is_empty() {
            create_dir_all(parent)?;
        }
    }
    let file = OpenOptions::new().create(true).append(true).open(path)?;
    let writer = SharedFileWriter {
        file: Arc::new(Mutex::new(file)),
    };
    let filter = EnvFilter::try_new(level).unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .with_target(true)
        .with_writer(writer)
        .compact()
        .init();
    Ok(())
}

pub fn line_has_required_fields(line: &str) -> bool {
    line.contains("trace_id=") && line.contains("episode_id=") && line.contains("worker_id=")
}

#[cfg(test)]
mod tests {
    use super::line_has_required_fields;

    #[test]
    fn parse_log_line_required_fields() {
        let line = "2026-05-26T22:19:00+08:00 INFO uenv.worker trace_id=t-1 episode_id=ep-1 worker_id=w-1 msg=\"dispatch\"";
        assert!(line_has_required_fields(line));
        assert_eq!(line.lines().count(), 1);
    }
}
