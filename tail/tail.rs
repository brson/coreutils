#![crate_id(name="tail", vers="1.0.0", author="Brian Anderson")]
#![feature(macro_rules)]

extern crate collections;
extern crate getopts;

use collections::{Deque, RingBuf};
use std::io::{File, BufferedReader, IoResult, SeekSet, SeekEnd};
use std::os;

#[path = "../common/util.rs"]
mod util;

static NAME: &'static str = "tail";

fn print_usage(opts: &[getopts::OptGroup]) {
    println!("tail 1.0.0\n");
    println!("Usage:\n  tail [OPTION]... [FILE]...\n");
    println!("{:s}", getopts::usage("Print sequences of numbers", opts));
}

#[allow(dead_code)]
fn main() { os::set_exit_status(uumain(os::args())); }

pub fn uumain(args: Vec<String>) -> int {

    let (args, odd_opts) = match preprocess_args(args) {
        Ok(r) => r,
        Err(e) => return e
    };

    let opts = [
        getopts::optopt("c", "bytes", "Output the last N bytes", "N"),
        getopts::optopt("n", "lines", "Output the last N lines", "N"),
        getopts::optflag("h", "help", "Print this help text and exit"),
        getopts::optflag("V", "version", "Print version and exit")
        ];
    let matches = match getopts::getopts(args.tail(), opts) {
        Ok(m) => m,
        Err(f) => {
            show_error!("{:s}", f.to_err_msg());
            print_usage(opts);
            return 1;
        }
    };
    if matches.opt_present("help") {
        print_usage(opts);
        return 0;
    }
    if matches.opt_present("version") {
        println!("tail 1.0.0");
        return 0;
    }

    // Build the configuration
    let config = config_from_matches(matches, odd_opts);

    match run(&config) {
        Ok(()) => 0,
        Err(e) => e
    }
}

struct OddOpts {
    // The "-1", "+2", etc. argument
    mode: Option<Mode>
}

struct Config {
    mode: Mode,
    files: Vec<String>,
    // When displaying multiple files, `tail` labels them
    print_headers: bool
}

struct Mode {
    unit: Unit,
    anchor: Anchor,
    count: uint
}

enum Unit { Bytes, Lines }
enum Anchor { FromBeginning, FromEnd }

// Parse some arguments that getopts can't handle
fn preprocess_args(args: Vec<String>) -> Result<(Vec<String>, OddOpts), int> {
    let mut mode = None;
    let mut success = true;
    let mut just_saw_dash_c_or_n = false;

    let mut i = 0;
    let args = args.move_iter().filter(|arg| {
        // The `-1`, etc. argument can only appear as the first argument
        let valid_count_position = i == 1;
        i += 1;

        let mut r = true;

        // If this special +1, -1 arg follows -c or -n, then getopts
        // will deal with those options correctly and we don't need any
        // special parsing.
        if !just_saw_dash_c_or_n {
            match parse_count(arg.as_slice()) {
                Some((anchor, count)) => {
                    if valid_count_position {
                        mode = Some(Mode {
                            unit: Lines,
                            anchor: anchor,
                            count: count
                        })
                    } else {
                        show_error!("tail: option used in wrong position -- {}", count);
                        success = false;
                    }
                    r = false
                }
                None => r = true
            }
        }

        if arg.equiv(&"-c") || arg.equiv(&"-n") {
            just_saw_dash_c_or_n = true
        } else {
            just_saw_dash_c_or_n = false
        }

        r
    }).collect();

    let odd_opts = OddOpts {
        mode: mode
    };

    if success {
        Ok((args, odd_opts))
    } else {
        Err(1)
    }
}

fn parse_count(arg: &str) -> Option<(Anchor, uint)> {
    let maybe_anchor = if arg.len() > 1 && arg.as_slice()[0] == '-' as u8 {
        Some(FromEnd)
    } else if arg.len() > 1 && arg.as_slice()[0] == '+' as u8 {
        Some(FromBeginning)
    } else {
        None
    };
    if maybe_anchor.is_some() {
        let anchor = maybe_anchor.unwrap();
        let maybe_number = arg.as_slice().slice(1, arg.len());
        match from_str(maybe_number) {
            Some(number) => Some((anchor, number)),
            None => None
        }
    } else {
        None
    }
}

fn config_from_matches(matches: getopts::Matches, odd_opts: OddOpts) -> Config {
    let default_mode = Mode { unit: Lines, anchor: FromEnd, count: 10 };

    let mut mode = {
        let maybe_n_str = matches.opt_str("n");
        match maybe_n_str {
            Some(count_str) => match parse_count(count_str.as_slice()) {
                Some((anchor, count)) => Mode {
                    unit: Lines,
                    anchor: anchor,
                    count: count
                },
                None => {
                    // FIXME: Shouldn't ignore option parse errors
                    default_mode
                }
            },
            None => {
                // No arguments to `n` provided
                default_mode
            }
        }
    };

    // FIXME: This option is incompatible with lots of others. Need to error
    if odd_opts.mode.is_some() {
        mode = odd_opts.mode.unwrap();
    }

    let files = matches.free.clone();
    let print_headers = files.len() > 1;

    Config {
        mode: mode,
        files: files,
        print_headers: print_headers
    }
}

fn run(config: &Config) -> Result<(), int> {

    let mut first_time = true;

    for path in config.files.iter() {
        if !first_time { println!("") }
        if config.print_headers {
            println!("==> {} <==", path);
        }
        match tail_file(path, config.mode) {
            Ok(()) => (),
            Err(_) => return Err(1),
        }
        first_time = false;
    }

    if config.files.len() == 0 {
        // If there are no files to tail then we're tailing stdin
        match tail_stdin(config.mode) {
            Ok(()) => (),
            Err(_) => return Err(1),
        }
    }

    Ok(())
}

fn tail_file(path: &String, mode: Mode) -> IoResult<()> {

    // TODO: Implement all modes for files; currently deferring
    // to tail_stream for unsupported modes
    if mode.unit != Lines || mode.anchor != FromEnd {
        let stream = try!(File::open(&Path::new(path.as_slice())));
        let mut buf_stream = BufferedReader::new(stream);
        return tail_stream(&buf_stream, mode);
    }

    let mut line_offsets = vec![];
    let mut next_offset = 0u64;

    let stream = try!(File::open(&Path::new(path.as_slice())));
    let mut buf_stream = BufferedReader::new(stream);
    for line in buf_stream.lines() {
        let line = try!(line);
        line_offsets.push(next_offset);
        next_offset += line.as_bytes().len() as u64;
    }

    let num_offsets = line_offsets.len();
    let first_line_offset = if num_offsets < mode.count { 0 }
                            else { *line_offsets.get(num_offsets - mode.count) };

    let mut stream = buf_stream.unwrap();
    if first_line_offset as i64 >= 0 {
        try!(stream.seek(first_line_offset as i64, SeekSet));
    } else {
        try!(stream.seek(first_line_offset as i64, SeekEnd));
    }
    let mut buf_stream = BufferedReader::new(stream);

    for line in buf_stream.lines() {
        let line = try!(line);
        print!("{}", line);
    }

    Ok(())
}

fn tail_stdin(mode: Mode) -> IoResult<()> {
    let mut stream = std::io::stdin();
    tail_stream(&mut stream, mode)
}

fn tail_stream<R: Reader>(stream: &mut BufferedReader<R>, mode: Mode) -> IoResult<()> {
    if mode.unit == Lines {
        let mut deque = RingBuf::with_capacity(mode.count);

        for line in stream.lines() {
            let line = try!(line);
            if deque.len() == mode.count {
                deque.pop_front();
            }
            deque.push_back(line);
        }

        loop {
            match deque.pop_front() {
                Some(line) => print!("{}", line),
                None => break
            }
        }
    } else {
    }

    Ok(())
}
