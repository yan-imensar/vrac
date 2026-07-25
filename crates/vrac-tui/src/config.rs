//! Small, user-owned terminal presentation configuration.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

const FILE_NAME: &str = "config.toml";

#[derive(Debug)]
pub(crate) struct Config {
    path: PathBuf,
    pub(crate) lines: bool,
}

impl Config {
    pub(crate) fn load() -> io::Result<Self> {
        Self::load_from(config_path()?)
    }

    fn load_from(path: PathBuf) -> io::Result<Self> {
        let contents = match fs::read_to_string(&path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(Self { path, lines: true });
            }
            Err(error) => {
                return Err(io::Error::new(
                    error.kind(),
                    format!("cannot read config {}: {error}", path.display()),
                ));
            }
        };
        let lines = parse(&path, &contents)?;
        Ok(Self { path, lines })
    }

    pub(crate) fn set_lines(&mut self, enabled: bool) -> io::Result<()> {
        let parent = self.path.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("config path has no parent: {}", self.path.display()),
            )
        })?;
        fs::create_dir_all(parent)?;

        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        writeln!(temporary, "lines = {enabled}")?;
        temporary.as_file().sync_all()?;
        temporary.persist(&self.path).map_err(|error| error.error)?;
        self.lines = enabled;
        Ok(())
    }
}

fn parse(path: &Path, contents: &str) -> io::Result<bool> {
    let mut lines = None;
    for (index, raw_line) in contents.lines().enumerate() {
        let line_number = index + 1;
        let line = raw_line
            .split_once('#')
            .map_or(raw_line, |(before, _)| before)
            .trim();
        if line.is_empty() {
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| invalid_config(path, line_number, "expected `name = value`"))?;
        let key = key.trim();
        if key != "lines" {
            return Err(invalid_config(
                path,
                line_number,
                &format!("unknown setting `{key}`"),
            ));
        }
        if lines.is_some() {
            return Err(invalid_config(
                path,
                line_number,
                "`lines` is configured more than once",
            ));
        }
        lines = Some(match value.trim() {
            "true" => true,
            "false" => false,
            _ => {
                return Err(invalid_config(
                    path,
                    line_number,
                    "`lines` must be `true` or `false`",
                ));
            }
        });
    }
    Ok(lines.unwrap_or(true))
}

fn invalid_config(path: &Path, line: usize, message: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!(
            "invalid config {} at line {line}: {message}",
            path.display()
        ),
    )
}

#[cfg(not(windows))]
fn config_path() -> io::Result<PathBuf> {
    if let Some(directory) = std::env::var_os("XDG_CONFIG_HOME")
        && !directory.is_empty()
    {
        let directory = PathBuf::from(directory);
        if !directory.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "XDG_CONFIG_HOME must be an absolute path",
            ));
        }
        return Ok(directory.join("vrac").join(FILE_NAME));
    }
    dirs::home_dir()
        .map(|home| home.join(".config").join("vrac").join(FILE_NAME))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "cannot determine home directory"))
}

#[cfg(windows)]
fn config_path() -> io::Result<PathBuf> {
    dirs::config_dir()
        .map(|directory| directory.join("vrac").join(FILE_NAME))
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "cannot determine the configuration directory",
            )
        })
}

#[cfg(test)]
mod tests {
    use super::Config;

    #[test]
    fn absent_config_uses_lines_without_creating_a_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("vrac").join("config.toml");

        let config = Config::load_from(path.clone()).unwrap();

        assert!(config.lines);
        assert!(!path.exists());
    }

    #[test]
    fn loads_comments_and_an_explicit_lines_value() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        std::fs::write(&path, "# Vrac presentation\nlines = false # compact\n").unwrap();

        assert!(!Config::load_from(path).unwrap().lines);
    }

    #[test]
    fn reports_the_path_and_line_of_invalid_config() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        std::fs::write(&path, "lines = true\ntheme = dark\n").unwrap();

        let error = Config::load_from(path.clone()).unwrap_err().to_string();

        assert!(error.contains(&path.display().to_string()));
        assert!(error.contains("line 2"));
        assert!(error.contains("unknown setting `theme`"));
    }

    #[test]
    fn set_lines_creates_and_atomically_replaces_the_config() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("nested").join("config.toml");
        let mut config = Config::load_from(path.clone()).unwrap();

        config.set_lines(false).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "lines = false\n");
        assert!(!Config::load_from(path.clone()).unwrap().lines);

        config.set_lines(true).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "lines = true\n");
        assert!(Config::load_from(path).unwrap().lines);
    }
}
