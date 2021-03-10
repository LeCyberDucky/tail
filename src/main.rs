// Feature ideas
// 1. Option for time stamps
// 2. Option for monitoring multiple files simultaneously
// 3. Option to read from top instead of bottom
// 4. Option to clear output
// 5. Other stuff from UNIX tail: https://en.wikipedia.org/wiki/Tail_(Unix)
// 6. Take refresh rate as optional argument
// 7. Handle Ctrl+C gracefully? https://rust-cli.github.io/book/in-depth/signals.html

// TODO:
// 1. Figure something out to handle double fired events
// Perhaps this is not too important? Just operate on duplicate events too, since there will just be nothing new to read. Let's hope content isn't deleted in the mean time, however.

#![feature(destructuring_assignment)]

use std::{
    collections::VecDeque,
    fs::OpenOptions,
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use anyhow::anyhow;
use anyhow::{Context, Result};
use clap::{App, Arg};
use crossbeam_utils::atomic::AtomicCell;
use hotwatch::{Event, Hotwatch};
use path_absolutize::*;
use thiserror::Error;

type Line = (usize, String);

#[derive(Debug, Error)]
enum FileError {
    #[error("Unable to access file: \"{path}\"")]
    Access {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("Unable to read line: {error_line}")]
    Read {
        valid_reads: Vec<Line>,
        error_line: usize,
        source: std::io::Error,
    },
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

fn main() -> Result<()> {
    let matches = App::new("tail")
        .version("1.0")
        .author("Andy")
        .about("Monitors a file, continuously printing new lines written to it")
        .arg(
            Arg::with_name("n")
                .short("n")
                .case_insensitive(true)
                .takes_value(true)
                .default_value("10")
                .default_value_if("follow", None, "1")
                .validator(|value| {
                    let value = value.parse::<usize>();
                    match value {
                        Ok(_) => Ok(()),
                        Err(_) => Err("n should be a positive integer".to_string()),
                    }
                })
                .value_name("NUMBER")
                // .conflicts_with("follow")
                .required(false)
                .help("The number of lines to display"),
        )
        .arg(
            Arg::with_name("follow")
                .short("f")
                .case_insensitive(true)
                .long("follow")
                .case_insensitive(true)
                .takes_value(false)
                .required(false)
                .help("Continuously monitor the file for new lines"),
        )
        .arg(
            Arg::with_name("file")
                .takes_value(true)
                .value_name("FILE")
                .required(true)
                .help("The file to monitor"),
        )
        .arg(
            Arg::with_name("rate")
                .long("rate")
                .case_insensitive(true)
                .takes_value(true)
                .default_value("60")
                .validator(|value| {
                    let value = value.parse::<f64>();
                    if let Ok(number) = value {
                        if number > 0.0 {
                            return Ok(());
                        }
                    }

                    Err("rate should be a positive number".to_string())
                })
                .value_name("NUMBER")
                .required(false)
                .help("Program logic refresh rate in Hz -> How often to check for file updates"),
        )
        .arg(
            Arg::with_name("delay")
                .long("delay")
                .case_insensitive(true)
                .takes_value(true)
                .default_value("100")
                .validator(|value| match value.parse::<u64>() {
                    Ok(_) => Ok(()),
                    Err(_) => Err("Delay should be a non-negative 64-bit integer.".to_string()),
                })
                .value_name("NUMBER")
                .required(false)
                .help("Program logic refresh rate in Hz -> How often to check for file updates"),
        )
        .arg(
            Arg::with_name("head")
                .long("head")
                .case_insensitive(true)
                .takes_value(false)
                .required(false)
                .help("Read from the beginning of the file"),
        )
        .arg(
            Arg::with_name("reverse")
                .short("r")
                .case_insensitive(true)
                .long("reverse")
                .case_insensitive(true)
                .takes_value(false)
                .required(false)
                .help("Print lines in reverse direction"),
        )
        .get_matches();

    let clock = Instant::now();

    let mut refresh_count = 0;
    let refresh_rate = matches.value_of("rate").unwrap().parse::<f64>().unwrap(); // Unwraps here are okay, I guess, because this has a default value and a validator

    let notification_delay = matches.value_of("delay").unwrap().parse::<u64>().unwrap(); // Unwraps here are okay, I guess, because this has a default value and a validator

    let reverse_flag = matches.is_present("reverse");

    let n = matches.value_of("n").unwrap().parse::<usize>().unwrap(); // Unwraps are safe because argument has validator and default value

    let (mut start_position, mut stop_position, reading_direction) = if matches.is_present("head") {
        (
            Position::FromBegin(0),
            Position::FromBegin(n),
            ReadingDirection::TopToBottom,
        )
    } else {
        (
            Position::FromEnd(0),
            Position::FromEnd(n),
            ReadingDirection::BottomToTop,
        )
    };

    // Parse input argument as file path
    let file_path = matches.value_of("file").unwrap(); // The unwrap here is safe, because the argument is required
    let mut file_path = validate_path(file_path);

    // Try to handle possible errors
    file_path = match file_path {
        Ok(path) => Ok(path),
        Err(error) => {
            match error {
                FileError::Access {
                    ref path,
                    source: _,
                } => {
                    eprintln!("{}\n{:#?}", error, error);
                    println!("Waiting for file to become accessible");

                    while OpenOptions::new().read(true).open(path.clone()).is_err() {
                        sleep_remaining_frame(clock, &mut refresh_count, refresh_rate);
                        todo!();
                    }

                    Ok(path.clone())
                }
                FileError::Read {
                    valid_reads: _,
                    error_line: _,
                    source: _,
                } => Err(error), // Don't think this case should happen, as we are not trying to read here
                FileError::Other(_) => Err(error),
            }
        }
    };

    // If error can't be handled, return
    let file_path = file_path?;

    // Read once, and then monitor if wanted
    let mut file = OpenOptions::new()
        .read(true)
        .open(file_path.clone())
        .map_err(|error| FileError::Access {
            path: file_path.clone(),
            source: error,
        })?;

    let lines = read_lines(&mut file, start_position, stop_position, reading_direction)?;
    let mut last_read_line = match reading_direction {
        // ReadingDirection::TopToBottom => lines.last().map(|(number, _)| *number).unwrap_or(0),
        // ReadingDirection::BottomToTop => lines.first().map(|(number, _)| *number).unwrap_or(0),
        ReadingDirection::TopToBottom => lines.last().cloned(),
        ReadingDirection::BottomToTop => lines.first().cloned(),
    };
    print_lines(lines, reading_direction, reverse_flag);

    if matches.occurrences_of("follow") > 0 {
        // Monitor continuously
        let file_changed = Arc::new(AtomicCell::new(false));

        let mut file_watcher = Hotwatch::new_with_custom_delay(Duration::from_millis(
            notification_delay,
        ))
        .context(format!(
            "Hotwatch failed to initialize. Unable to monitor {:?}!",
            file_path
        ))?;

        {
            let file_changed = Arc::clone(&file_changed);

            println!("Watching! (⌐■_■)");
            file_watcher
                .watch(&file_path, move |event| {
                    if let Event::Write(_path) = event {
                        file_changed.store(true);
                    }
                })
                .context(format!("Failed to watch {:?}!", file_path))?;
        }

        loop {
            // Monitor file
            if file_changed.compare_exchange(true, false).is_ok() {
                match reading_direction {
                    ReadingDirection::TopToBottom => {
                        todo!();
                    }
                    ReadingDirection::BottomToTop => {
                        (start_position, stop_position) =
                            (Position::FromEnd(0), Position::FromBegin(0)); // stop_position is FromBegin(0), since the curser is where we left it
                    }
                }

                let mut lines =
                    read_lines(&mut file, start_position, stop_position, reading_direction)?;

                let mut previous_last_read_line = last_read_line.clone();

                if let Some((last_line_number, last_line_content)) = &mut last_read_line {
                    if !last_line_content.ends_with('\n') {
                        // Previous last line did not include newline characters. These are read as their own line now
                        match reading_direction {
                            ReadingDirection::TopToBottom => {
                                if let Some((_, line)) = lines.first() {
                                    if line == "\r\n" || line == "\n" {
                                        // Consider this part of the last read line
                                        if let Some((number, mut string)) = previous_last_read_line
                                        {
                                            string.push_str(line);
                                            previous_last_read_line = Some((number, string));
                                        };

                                        lines.remove(0);

                                        for (line_number, _) in &mut lines {
                                            *line_number += *last_line_number - 1;
                                            // - 1 because the new line ending on the previous last line shoult not be counted as an individual new line
                                        }
                                    }
                                }
                            }
                            ReadingDirection::BottomToTop => {
                                if let Some((_, line)) = lines.last() {
                                    if line == "\r\n" || line == "\n" {
                                        // Consider this part of the last read line
                                        if let Some((number, mut string)) = previous_last_read_line
                                        {
                                            string.push_str(line);
                                            previous_last_read_line = Some((number, string));
                                        };

                                        lines.remove(lines.len() - 1);

                                        for (line_number, _) in &mut lines {
                                            *line_number += *last_line_number - 1;
                                            // - 1 because the new line ending on the previous last line should not be counted as an individual new line
                                        }
                                    }
                                }
                            }
                        }
                    } else {
                        for (line_number, _) in &mut lines {
                            *line_number += *last_line_number;
                        }
                    }
                }

                match reading_direction {
                    ReadingDirection::TopToBottom => {
                        if lines.last().is_some() {
                            last_read_line = lines.last().cloned();
                        } else {
                            last_read_line = previous_last_read_line;
                        }
                    }
                    ReadingDirection::BottomToTop => {
                        if lines.first().is_some() {
                            last_read_line = lines.first().cloned();
                        } else {
                            last_read_line = previous_last_read_line;
                        }
                    }
                };

                print_lines(lines, reading_direction, reverse_flag);
            }

            sleep_remaining_frame(clock, &mut refresh_count, refresh_rate);
        }
    }

    Ok(())
}

#[derive(Debug, PartialEq, Clone, Copy)]
enum ReadingDirection {
    TopToBottom,
    BottomToTop,
}

#[derive(Debug, PartialEq, Clone, Copy)]
enum Position {
    FromBegin(usize),
    FromEnd(usize),
}

fn read_lines<Readable: Read>(
    data: Readable,
    mut start: Position,
    mut stop: Position,
    direction: ReadingDirection,
) -> std::result::Result<Vec<Line>, FileError> {
    match direction {
        ReadingDirection::TopToBottom => match (start, stop) {
            (Position::FromBegin(a), Position::FromBegin(b)) => {
                if a >= b {
                    return Ok(vec![]);
                }
            }
            (Position::FromBegin(_), Position::FromEnd(_)) => {}
            (Position::FromEnd(_), Position::FromBegin(_)) => {}
            (Position::FromEnd(a), Position::FromEnd(b)) => {
                if a <= b {
                    return Ok(vec![]);
                }
            }
        },
        ReadingDirection::BottomToTop => match (start, stop) {
            (Position::FromBegin(a), Position::FromBegin(b)) => {
                if a <= b {
                    return Ok(vec![]);
                } else {
                    (start, stop) = (stop, start);
                }
            }
            (Position::FromBegin(_), Position::FromEnd(_)) => {
                (start, stop) = (stop, start);
            }
            (Position::FromEnd(_), Position::FromBegin(_)) => {
                (start, stop) = (stop, start);
            }
            (Position::FromEnd(a), Position::FromEnd(b)) => {
                if a >= b {
                    return Ok(vec![]);
                } else {
                    (start, stop) = (stop, start);
                }
            }
        },
    }

    let mut reader = BufReader::new(data);

    let mut lines = VecDeque::new();
    let mut line_count = 0;
    let mut line_buffer = String::new();

    // Keep on reading
    loop {
        // When to store line?
        // -> If start is FromBegin(pos) and line_count >= pos
        // -> If start is FromEnd (since we don't know the total line count before hand)
        // When to stop?
        // -> If stop is FromBegin(pos) and line_count >= pos
        // -> If end of file has been reached

        // Check for stop condition
        if let Position::FromBegin(pos) = stop {
            if line_count >= pos {
                break;
            }
        }

        line_buffer.clear();
        let bytes_read = reader.read_line(&mut line_buffer);
        line_count += 1;

        match bytes_read {
            Ok(count) => {
                if count == 0 {
                    // End of file reached
                    break;
                }
            }
            Err(error) => {
                return Err(FileError::Read {
                    valid_reads: match direction {
                        ReadingDirection::TopToBottom => lines.into(),
                        ReadingDirection::BottomToTop => {
                            lines.into_iter().rev().collect::<Vec<(usize, String)>>()
                        }
                    },
                    error_line: line_count,
                    source: error,
                })
            }
        }

        // Only store line if wanted starting position has been passed
        if let Position::FromBegin(pos) = start {
            if line_count < pos {
                continue;
            }
        }

        lines.push_back((line_count, line_buffer.clone()));

        // Drop lines making the container larger than the greatest pos given in a FromEnd(pos)
        match (start, stop) {
            (Position::FromBegin(a), Position::FromBegin(b)) => {
                if lines.len() > b - a {
                    lines.pop_front();
                }
            }
            (Position::FromBegin(_), Position::FromEnd(_)) => {}
            (Position::FromEnd(a), Position::FromBegin(_)) => {
                if lines.len() > a {
                    lines.pop_front();
                }
            }
            (Position::FromEnd(a), Position::FromEnd(_)) => {
                if lines.len() > a {
                    lines.pop_front();
                }
            }
        }
    }

    // Remove lines towards end of file that shouldn't be included
    if let Position::FromEnd(n) = stop {
        lines.drain(lines.len().saturating_sub(n)..);
    }

    match direction {
        ReadingDirection::TopToBottom => Ok(lines.into()),
        ReadingDirection::BottomToTop => {
            Ok(lines.into_iter().rev().collect::<Vec<(usize, String)>>())
        }
    }

    // https://crates.io/crates/easy_reader
    // https://www.reddit.com/r/rust/comments/99e4tq/reading_files_quickly_in_rust/
    // https://github.com/Freaky/rust-linereader
    // https://www.reddit.com/r/rust/comments/99lm5l/easyreader_an_easy_and_fast_way_to_read_huge/
    // https://codereview.stackexchange.com/questions/227204/fast-text-search-in-rust
    // https://doc.rust-lang.org/std/io/trait.BufRead.html#method.read_line
    // https://www.reddit.com/r/rust/comments/8833lh/performance_of_parsing_large_file_2gb/
    // https://depth-first.com/articles/2020/07/20/reading-sd-files-in-rust/
    // https://stackoverflow.com/questions/31986628/collect-items-from-an-iterator-at-a-specific-index
}

fn print_lines(
    mut lines: Vec<(usize, String)>,
    reading_direction: ReadingDirection,
    reverse_flag: bool,
) {
    if reading_direction == ReadingDirection::BottomToTop {
        lines = lines.into_iter().rev().collect();
    }

    if reverse_flag {
        for (line_number, line) in lines.iter().rev() {
            print!("{}:\t{}", line_number, line);
            if !line.ends_with('\n') {
                println!();
            }
        }
    } else {
        for (line_number, line) in lines.iter() {
            print!("{}:\t{}", line_number, line);
            if !line.ends_with('\n') {
                println!();
            }
        }
    }
}

fn validate_path(path_string: &str) -> std::result::Result<PathBuf, FileError> {
    let mut path = path_string.to_string();
    if path.trim().is_empty() {
        return Err(FileError::Other(anyhow!("Supplied path is empty!")));
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
        .with_context(|| format!("Unable to turn \"{}\" into absolute path", path))?;

    if path.is_dir() {
        return Err(FileError::Other(anyhow!(
            "The path \"{}\" points to a directory. It should point to a file",
            path.to_str().unwrap_or("")
        )));
    }

    let file = OpenOptions::new().read(true).open(path.clone());
    match file {
        Ok(_) => Ok(path.into()),
        Err(error) => Err(FileError::Access {
            path: path.into(),
            source: error,
        }),
    }
}

fn sleep_remaining_frame(clock: Instant, count: &mut u128, rate: f64) {
    *count += 1;

    let micros_per_second = 1_000_000;
    let expected_frame_count = (clock.elapsed().as_micros() as f64 * rate) as u128;
    let frame_count = *count * micros_per_second;

    // If this is positive, we should sleep the difference away
    let count_delta = (frame_count as i128) - (expected_frame_count as i128);

    if count_delta > 0 {
        let sleep_time = ((count_delta as f64) / rate) as u128;
        thread::sleep(Duration::from_micros(sleep_time as u64));
    }
}

mod tests {
    use super::*;

    #[test]
    fn test_read_lines() -> Result<()> {
        let file = r"In Hamburg lebten zwei Ameisen,
        Die wollten nach Australien reisen.
        Bei Altona auf der Chaussee
        Da taten ihnen die Beine weh,
        Und da verzichteten sie weise
        Denn auf den letzten Teil der Reise.
        
        So will man oft und kann doch nicht
        Und leistet dann recht gern Verzicht."
            .to_string();

        let data = file.clone();
        let (a, b) = (0, 7);
        let (start, stop) = (Position::FromBegin(a), Position::FromBegin(b));
        let direction = ReadingDirection::TopToBottom;
        let lines = read_lines(data.as_bytes(), start, stop, direction)?;
        let expected: Vec<(usize, String)> = (a..b)
            .map(|i| (i + 1, data.lines().nth(i).unwrap().to_string() + "\n"))
            .collect();

        assert_eq!(lines, expected);
        Ok(())
    }
}
