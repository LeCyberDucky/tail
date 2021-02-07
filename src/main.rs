// Feature ideas
// 1. Option for time stamps
// 2. Option for monitoring multiple files simultaneously
// 3. Option to read from top instead of bottom
// 4. Option to clear output

use std::fs::OpenOptions;
use std::path::{Path, PathBuf};

use anyhow::anyhow;
use anyhow::{Context, Result};
use clap::{App, Arg};
use path_absolutize::*;
use thiserror::Error;

#[derive(Debug, Error)]
enum FileError {
    #[error("Unable to access file: \"{path}\"")]
    AccessError {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error(transparent)]
    OtherError(#[from] anyhow::Error),
}

fn main() -> Result<()> {
    let matches = App::new("tail")
        .version("1.0")
        .author("Andy")
        .about("Monitors a file, continuously printing new lines written to it")
        .arg(
            Arg::with_name("file")
                .short("f")
                .value_name("FILE")
                .help("The file to monitor")
                .required(true)
                .index(1)
                .takes_value(true),
        )
        .get_matches();

    // Parse input argument as file path
    let file = matches.value_of("file").unwrap(); // The unwrap here is safe, because the argument is required
    let file = validate_path(file);

    let file_path;

    match file {
        Ok(path) => file_path = path,
        Err(error) => match error {
            FileError::AccessError { path: _, source: _ } => {
                eprintln!("{}\n{:#?}", error, error);
                println!("Waiting for file to become accessible.");

                if let FileError::AccessError { path, source: _ } = error {
                    file_path = path;
                    while !OpenOptions::new()
                        .read(true)
                        .open(file_path.clone())
                        .is_ok()
                    {
                        // sleep_remaining_frame();
                        todo!();
                    }
                }
            }
            FileError::OtherError(error) => return Err(error),
        },
    }

    loop {
        // Check for file change
        // Print change
        // Sleep
    }

    // Keep reading/printing

    // No need to check for command line input, since ctrl+Z will just naturally terminate the program

    Ok(())
}

fn validate_path(path_string: &str) -> std::result::Result<PathBuf, FileError> {
    let mut path = path_string.to_string();
    if path.trim().is_empty() {
        return Err(FileError::OtherError(anyhow!("Supplied path is empty!")));
    }

    // If the path is relative, trim it and add "./" to the beginning
    let trim_characters = ['\\', '/', '.'];
    if Path::new(&path).is_relative() {
        let first_character = path.chars().next().unwrap(); // At least one character is contained, as given by the check above
        if first_character != '.' {
            path = path
                .trim_start_matches(|c: char| c.is_whitespace() || trim_characters.contains(&c))
                .to_string();
            path.insert_str(0, "./");
        }
    }

    let path = Path::new(&path)
        .absolutize()
        .with_context(|| format!("Unable to turn \"{}\" into absolute path.", path))?;

    if path.is_dir() {
        return Err(FileError::OtherError(anyhow!(
            "The path \"{}\" points to a directory. It should point to a file.",
            match path.to_str() {
                Some(str) => str,
                None => "",
            }
        )));
    }

    let file = OpenOptions::new().read(true).open(path.clone());
    match file {
        Ok(_) => Ok(path.into()),
        Err(error) => Err(FileError::AccessError {
            path: path.into(),
            source: error,
        }),
    }
}
