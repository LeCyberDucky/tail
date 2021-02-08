// Feature ideas
// 1. Option for time stamps
// 2. Option for monitoring multiple files simultaneously
// 3. Option to read from top instead of bottom
// 4. Option to clear output
// 5. Other stuff from UNIX tail: https://en.wikipedia.org/wiki/Tail_(Unix)
// 6. Take refresh rate as optional argument

use std::{fs::OpenOptions, io::BufRead};
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

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
            Arg::with_name("n")
                .short("-n")
                .case_insensitive(true)
                .takes_value(true)
                .default_value("10")
                .validator(|value| {
                    let value = value.parse::<usize>();
                    match value {
                        Ok(_) => Ok(()),
                        Err(_) => Err("n should be a positive integer.".to_string()),
                    }
                })
                .value_name("NUMBER")
                .conflicts_with("follow")
                .required(false)
                .help("The number of lines to display."),
        )
        .arg(
            Arg::with_name("follow")
                .short("-f")
                .case_insensitive(true)
                .long("-follow")
                .case_insensitive(true)
                .takes_value(false)
                .required(false)
                .help("Continuously monitor the file for new lines."),
        )
        .arg(
            Arg::with_name("file")
                .takes_value(true)
                .value_name("FILE")
                .help("The file to monitor")
                .required(true),
        ).arg(
            Arg::with_name("rate")
                .case_insensitive(true)
                .takes_value(true)
                .default_value("4")
                .validator(|value| {
                    let value = value.parse::<f64>();
                    if let Ok(number) = value {
                        if number > 0.0 {
                            return Ok(());
                        }
                    }
                    
                    Err("rate should be a positive number.".to_string())
                })
                .value_name("NUMBER")
                .required(false)
                .help("Refresh rate in Hz -> How often to check for file updates.")

        )
        .arg(
            Arg::with_name("head")
                .case_insensitive(true)
                .takes_value(false)
                .help("Read from top to bottom")
        )
        .get_matches();

    println!("{:?}", matches);

    // Parse input argument as file path
    let file = matches.value_of("file").unwrap(); // The unwrap here is safe, because the argument is required
    let file = validate_path(file);

    let mut clock = Instant::now();
    let mut refresh_count = 0;
    let refresh_rate = 10;

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
                        sleep_remaining_frame(clock, &mut refresh_count, refresh_rate);
                        todo!();
                    }
                }
            }
            FileError::OtherError(error) => return Err(error),
        },
    }

    if matches.occurrences_of("n") > 0 {
        // Only read once. Do not monitor continuously
    } else {
        // Monitor continuously
        clock = Instant::now();
        loop {
            if (clock.elapsed().as_secs() % 5) == 0 {
                println!(
                    "Elapsed seconds: {}\t Frame count: {}",
                    clock.elapsed().as_secs(),
                    refresh_count
                );
            }
            sleep_remaining_frame(clock, &mut refresh_count, refresh_rate);
        }
    }

    // If -f specified, continuously monitor file

    // loop {
    //     // Check for file change
    //     // Print change
    //     // Sleep
    // }

    // Keep reading/printing

    // No need to check for command line input, since ctrl+Z will just naturally terminate the program

    Ok(())
}

enum ReadingOrder {
    TopToBottom,
    BottomToTop,
}

enum Position {
    Begin,
    Inbetween(usize),
    End,
}

fn read_lines(file_path: PathBuf, order: ReadingOrder, start_position: Position, n: usize) -> Result<Vec<String>> {
    let file = OpenOptions::new()
                        .read(true)
                        .open(file_path)?;

    let file_buffer = BufReader::new(file);
    let lines: Result<Vec<String>, _> = file_buffer.lines().collect();
    let lines = lines?;
    let line_count = lines.len();

    let (start_position, id) = match start_position {
        Position::Begin => (Position::Inbetween(0), 0),
        Position::End => (Position::Inbetween(line_count), line_count),
        Position::Inbetween(pos) => (Position::Inbetween(pos), pos),
    };

    let stop_position = match order {
        ReadingOrder::TopToBottom => Position::Inbetween(
                (id + n).min(line_count)
            ),
        ReadingOrder::BottomToTop => Position::Inbetween(
                id.saturating_sub(n)
        ),
    };

    todo!();


    Ok(vec![])
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

fn sleep_remaining_frame(clock: Instant, count: &mut u128, rate: u128) {
    *count += 1;

    let micros_per_second = 1_000_000;
    let expected_frame_count = clock.elapsed().as_micros() * rate;
    let frame_count = *count * micros_per_second;

    // If this is positive, we should sleep the difference away
    let count_delta = (frame_count as i128) - (expected_frame_count as i128);

    if count_delta > 0 {
        let sleep_time = (count_delta as u128) / rate;
        thread::sleep(Duration::from_micros(sleep_time as u64));
    }
}
