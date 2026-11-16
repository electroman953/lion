//! The functions of the standard modules that the implementation provides (§23): the
//! computations come from `lion_runtime`; this file turns values into arguments and
//! results.

use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use lion_ir::Native;
use lion_runtime::BugKind;
use lion_runtime::{net, stdlib};

use crate::value::Value;

/// The result of a native function, or the bug it meets.
pub fn call(native: Native, args: &[Value]) -> Result<Value, BugKind> {
    let text = |index: usize| match &args[index] {
        Value::Text(text) => text.as_str(),
        other => {
            panic!("the native function {} expected a Text but found {}", native.name(), other.type_name())
        }
    };
    let int = |index: usize| match &args[index] {
        Value::Int(value) => *value,
        other => {
            panic!("the native function {} expected an Int but found {}", native.name(), other.type_name())
        }
    };
    let float = |index: usize| match &args[index] {
        Value::Float(value) => *value,
        other => {
            panic!("the native function {} expected a Float but found {}", native.name(), other.type_name())
        }
    };
    let texts = |values: Vec<String>| Value::List(Arc::new(values.into_iter().map(new_text).collect()));
    let error = |message: String| Value::Error(Arc::new(message));
    Ok(match native {
        Native::FilesRead => match std::fs::read_to_string(text(0)) {
            Ok(content) => new_text(content),
            Err(problem) => error(format!("cannot read `{}`: {}", text(0), describe(&problem))),
        },
        Native::FilesWrite => match std::fs::write(text(0), text(1)) {
            Ok(()) => Value::None,
            Err(problem) => error(format!("cannot write `{}`: {}", text(0), describe(&problem))),
        },
        Native::FilesExists => Value::Bool(std::path::Path::new(text(0)).exists()),
        Native::FilesAppend => {
            let appended = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(text(0))
                .and_then(|mut file| std::io::Write::write_all(&mut file, text(1).as_bytes()));
            match appended {
                Ok(()) => Value::None,
                Err(problem) => error(format!("cannot write `{}`: {}", text(0), describe(&problem))),
            }
        }
        Native::FilesList => {
            let names: std::io::Result<Vec<String>> = std::fs::read_dir(text(0)).and_then(|entries| {
                entries
                    .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
                    .collect()
            });
            match names {
                Ok(mut names) => {
                    names.sort();
                    texts(names)
                }
                Err(problem) if problem.kind() == std::io::ErrorKind::NotFound => {
                    error(format!("cannot list `{}`: the folder does not exist", text(0)))
                }
                Err(problem) => error(format!("cannot list `{}`: {}", text(0), describe(&problem))),
            }
        }
        Native::FilesIsFolder => Value::Bool(std::path::Path::new(text(0)).is_dir()),
        Native::FilesMakeFolder => match std::fs::create_dir_all(text(0)) {
            Ok(()) => Value::None,
            Err(problem) => error(format!("cannot create the folder `{}`: {}", text(0), describe(&problem))),
        },
        Native::FilesRemove => {
            let path = std::path::Path::new(text(0));
            let removed = if path.is_dir() { std::fs::remove_dir(path) } else { std::fs::remove_file(path) };
            match removed {
                Ok(()) => Value::None,
                Err(problem) if problem.kind() == std::io::ErrorKind::DirectoryNotEmpty => {
                    error(format!("cannot remove `{}`: the folder is not empty", text(0)))
                }
                Err(problem) => error(format!("cannot remove `{}`: {}", text(0), describe(&problem))),
            }
        }
        Native::FilesCopy => match std::fs::copy(text(0), text(1)) {
            Ok(_) => Value::None,
            Err(problem) => {
                error(format!("cannot copy `{}` to `{}`: {}", text(0), text(1), describe(&problem)))
            }
        },
        Native::FilesMove => match std::fs::rename(text(0), text(1)) {
            Ok(()) => Value::None,
            Err(problem) => {
                error(format!("cannot move `{}` to `{}`: {}", text(0), text(1), describe(&problem)))
            }
        },
        Native::SystemArgs => texts(program_args().to_vec()),
        Native::SystemEnv => std::env::var(text(0)).map_or(Value::None, new_text),
        Native::SystemExecute => {
            let arguments = text_list(&args[1]);
            match execute(text(0), &arguments, text(2)) {
                Ok(words) => texts(words),
                Err(message) => error(message),
            }
        }
        Native::SystemWindows => Value::Bool(cfg!(windows)),
        Native::NetFetch => {
            let headers = pairs_of(&text_list(&args[2]));
            match net::fetch(text(0), text(1), &headers, text(3), float(4)) {
                Ok(response) => {
                    let mut words = vec![response.status.to_string(), response.body];
                    flatten(response.headers, &mut words);
                    texts(words)
                }
                Err(message) => error(message),
            }
        }
        Native::NetSave => match net::download(text(0), text(1), float(2)) {
            Ok(()) => Value::None,
            Err(message) => error(message),
        },
        Native::NetListen => match net::listen(text(0), int(1)) {
            Ok(server) => Value::Int(server),
            Err(message) => error(message),
        },
        Native::NetPort => Value::Int(net::port(int(0))),
        Native::NetAccept => match net::accept(int(0)) {
            Ok(request) => {
                let mut words = vec![
                    request.connection.to_string(),
                    request.method,
                    request.path,
                    request.query,
                    request.body,
                ];
                flatten(request.headers, &mut words);
                texts(words)
            }
            Err(message) => error(message),
        },
        Native::NetReply => {
            let headers = pairs_of(&text_list(&args[2]));
            match net::respond(int(0), int(1), &headers, text(3)) {
                Ok(()) => Value::None,
                Err(message) => error(message),
            }
        }
        Native::NetPairs => {
            let mut words = Vec::new();
            flatten(net::form_pairs(text(0)), &mut words);
            texts(words)
        }
        Native::NetEncode => new_text(net::encode(text(0))),
        Native::TextSplit => {
            if text(1).is_empty() {
                return Err(BugKind::InvalidArgument {
                    message: "cannot split a text at an empty separator".to_string(),
                    details: "`text.split(t, \"\")` has no parts to give".to_string(),
                });
            }
            texts(text(0).split(text(1)).map(str::to_string).collect())
        }
        Native::TextJoin => {
            let Value::List(parts) = &args[0] else { panic!("text.join expected a List") };
            let parts: Vec<String> = parts.iter().map(Value::to_text).collect();
            new_text(parts.join(text(1)))
        }
        Native::TextUpper => new_text(text(0).to_uppercase()),
        Native::TextLower => new_text(text(0).to_lowercase()),
        Native::TextTrim => new_text(text(0).trim().to_string()),
        Native::TextContains => Value::Bool(text(0).contains(text(1))),
        Native::TextStartsWith => Value::Bool(text(0).starts_with(text(1))),
        Native::TextEndsWith => Value::Bool(text(0).ends_with(text(1))),
        Native::TextReplace => {
            if text(1).is_empty() {
                return Err(BugKind::InvalidArgument {
                    message: "cannot replace an empty text".to_string(),
                    details: "`text.replace(t, \"\", new)` would put `new` between every character"
                        .to_string(),
                });
            }
            new_text(text(0).replace(text(1), text(2)))
        }
        Native::TextFind => stdlib::text_find(text(0), text(1)).map_or(Value::None, Value::Int),
        Native::TextLines => texts(stdlib::text_lines(text(0))),
        Native::MathSqrt => Value::Float(float(0).sqrt()),
        Native::MathSin => Value::Float(float(0).sin()),
        Native::MathCos => Value::Float(float(0).cos()),
        Native::MathTan => Value::Float(float(0).tan()),
        Native::MathAsin => Value::Float(float(0).asin()),
        Native::MathAcos => Value::Float(float(0).acos()),
        Native::MathAtan => Value::Float(float(0).atan()),
        Native::MathAtan2 => Value::Float(float(0).atan2(float(1))),
        Native::MathExp => Value::Float(float(0).exp()),
        Native::MathLog => Value::Float(float(0).ln()),
        Native::MathLog10 => Value::Float(float(0).log10()),
        Native::RandomAdvance => Value::Int(stdlib::random_advance(int(0))),
        Native::RandomUnit => Value::Float(stdlib::random_unit(int(0))),
        Native::RandomBelow => match stdlib::random_below(int(0), int(1)) {
            Some(value) => Value::Int(value),
            None => {
                return Err(BugKind::InvalidArgument {
                    message: "no number to draw from an empty interval".to_string(),
                    details: format!("the interval holds {} numbers", int(1).max(0)),
                });
            }
        },
        Native::RandomSeed => {
            let nanos =
                SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_nanos() as i64);
            Value::Int(nanos ^ i64::from(std::process::id()))
        }
        Native::CsvParse => match stdlib::csv_parse(text(0)) {
            Ok(rows) => Value::List(Arc::new(rows.into_iter().map(texts).collect())),
            Err(message) => error(message),
        },
        Native::JsonRows => match lion_runtime::json::rows(text(0)) {
            Ok(rows) => Value::List(Arc::new(
                rows.into_iter()
                    .map(|row| {
                        Value::List(Arc::new(vec![
                            texts(row.columns),
                            texts(row.values),
                            texts(row.problems),
                        ]))
                    })
                    .collect(),
            )),
            Err(message) => error(message),
        },
        Native::UiOpen => {
            match lion_ui::open(text(0), int(1).clamp(1, 8192) as u32, int(2).clamp(1, 8192) as u32) {
                Ok(window) => Value::Int(window),
                Err(message) => error(message),
            }
        }
        Native::UiWidth => Value::Int(i64::from(lion_ui::size(int(0)).0)),
        Native::UiHeight => Value::Int(i64::from(lion_ui::size(int(0)).1)),
        Native::UiClear => {
            lion_ui::clear(int(0), color(int(1)));
            Value::None
        }
        Native::UiFill => {
            lion_ui::fill(int(0), int(1), int(2), int(3), int(4), color(int(5)));
            Value::None
        }
        Native::UiFrame => {
            lion_ui::frame(int(0), int(1), int(2), int(3), int(4), color(int(5)));
            Value::None
        }
        Native::UiWrite => {
            let Value::Bool(bold) = args[5] else { panic!("ui.write expected a Bool") };
            lion_ui::text(int(0), int(1), int(2), text(3), int(4), bold, color(int(6)));
            Value::None
        }
        Native::UiPresent => match lion_ui::present(int(0)) {
            Ok(()) => Value::None,
            Err(message) => error(message),
        },
        Native::UiNextEvent => {
            let timeout = u64::try_from(int(1)).ok();
            match lion_ui::next_event(int(0), timeout) {
                Ok(event) => texts(event_words(event)),
                Err(message) => error(message),
            }
        }
        Native::UiClose => {
            lion_ui::close(int(0));
            Value::None
        }
        Native::UiTextWidth => {
            let Value::Bool(bold) = args[2] else { panic!("ui.text_width expected a Bool") };
            Value::Int(lion_ui::text_width(text(0), int(1), bold))
        }
        Native::UiLineHeight => Value::Int(lion_ui::line_height(int(0))),
        Native::UiHeadless => Value::Bool(lion_ui::headless()),
        Native::UiReady => {
            let Value::Task(task) = &args[0] else { panic!("ui.ready expected a Task") };
            Value::Bool(task.is_ready())
        }
        Native::UiNap => {
            std::thread::sleep(Duration::from_millis(int(0).clamp(0, 1000) as u64));
            Value::None
        }
        Native::TimeNow => Value::Float(unix_seconds()),
        Native::TimeClock => {
            static ORIGIN: OnceLock<Instant> = OnceLock::new();
            Value::Float(ORIGIN.get_or_init(Instant::now).elapsed().as_secs_f64())
        }
        Native::TimeSleep => {
            let seconds = float(0);
            if !(seconds >= 0.0 && seconds.is_finite()) {
                return Err(BugKind::InvalidArgument {
                    message: "cannot wait for this duration".to_string(),
                    details: format!(
                        "{} is not a number of seconds from 0 up",
                        lion_runtime::format::format_float(seconds)
                    ),
                });
            }
            std::thread::sleep(Duration::from_secs_f64(seconds));
            Value::None
        }
        Native::DatesBeyond => {
            return Err(BugKind::InvalidArgument {
                message: "no date before 0001-01-01 or after 9999-12-31".to_string(),
                details: format!("the result would be {} days after 1970-01-01", int(0)),
            });
        }
        Native::DatesLocalDay => {
            let seconds = unix_seconds().floor() as i64;
            Value::Int((seconds + local_offset(seconds)).div_euclid(86_400))
        }
    })
}

/// The words written after the program. `lion run` gives those after the file; a
/// compiled program, those after its name.
pub fn set_program_args(args: Vec<String>) {
    let _ = PROGRAM_ARGS.set(args);
}

static PROGRAM_ARGS: OnceLock<Vec<String>> = OnceLock::new();

fn program_args() -> &'static [String] {
    PROGRAM_ARGS.get_or_init(|| std::env::args().skip(1).collect())
}

/// Runs `program` and gives its exit code, its output and its errors.
fn execute(program: &str, arguments: &[String], input: &str) -> Result<Vec<String>, String> {
    use std::io::Write as _;
    use std::process::{Command, Stdio};
    let mut child = Command::new(program)
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|problem| match problem.kind() {
            std::io::ErrorKind::NotFound => format!("cannot run `{program}`: the program does not exist"),
            _ => format!("cannot run `{program}`: {}", describe(&problem)),
        })?;
    let mut stdin = child.stdin.take().expect("piped");
    let input = input.to_string();
    // Written from another thread: a program that writes much before reading would
    // otherwise wait for us while we wait for it.
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(input.as_bytes());
    });
    let output = child.wait_with_output().map_err(|problem| format!("cannot run `{program}`: {problem}"))?;
    let _ = writer.join();
    // A program stopped by a signal has no code: -1.
    let code = output.status.code().unwrap_or(-1);
    Ok(vec![
        code.to_string(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    ])
}

/// The Texts of a `List of Text`.
fn text_list(value: &Value) -> Vec<String> {
    let Value::List(items) = value else { panic!("a native function expected a List of Text") };
    items.iter().map(Value::to_text).collect()
}

/// `[name, value, name, value…]` as pairs.
fn pairs_of(words: &[String]) -> Vec<(String, String)> {
    words.as_chunks::<2>().0.iter().map(|[name, value]| (name.clone(), value.clone())).collect()
}

fn flatten(pairs: Vec<(String, String)>, words: &mut Vec<String>) {
    for (name, value) in pairs {
        words.push(name);
        words.push(value);
    }
}

/// A color of the program, `0xRRGGBB`.
fn color(value: i64) -> u32 {
    (value & 0xFF_FF_FF) as u32
}

/// An event of the interface, as words that the module `ui` reads: its kind, then its
/// details (C98).
fn event_words(event: lion_ui::Event) -> Vec<String> {
    use lion_ui::{Event, Key};
    let words: Vec<String> = match event {
        Event::Close => vec!["close".into()],
        Event::Click { x, y } => vec!["click".into(), x.to_string(), y.to_string()],
        Event::Key(Key::Char(c)) => vec!["type".into(), c.to_string()],
        Event::Key(key) => vec!["key".into(), format!("{key:?}").to_lowercase()],
        Event::Resize { width, height } => vec!["resize".into(), width.to_string(), height.to_string()],
        Event::Redraw => vec!["redraw".into()],
        Event::Timeout => vec!["timeout".into()],
        Event::ClickText(text) => vec!["click-text".into(), text],
        Event::Type(text) => vec!["type".into(), text],
        Event::Wait => vec!["wait".into()],
    };
    words
}

/// The seconds since 1970-01-01 00:00 UTC, negative before.
fn unix_seconds() -> f64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(elapsed) => elapsed.as_secs_f64(),
        Err(before) => -before.duration().as_secs_f64(),
    }
}

/// How many seconds the local time is ahead of UTC at the moment `time`, from the time
/// zone of the system (`localtime_r`).
#[cfg(all(unix, target_pointer_width = "64"))]
fn local_offset(time: i64) -> i64 {
    use std::ffi::{c_char, c_int, c_long};
    #[repr(C)]
    struct Tm {
        sec: c_int,
        min: c_int,
        hour: c_int,
        mday: c_int,
        mon: c_int,
        year: c_int,
        wday: c_int,
        yday: c_int,
        isdst: c_int,
        gmtoff: c_long,
        zone: *const c_char,
    }
    unsafe extern "C" {
        fn localtime_r(time: *const i64, result: *mut Tm) -> *mut Tm;
    }
    let mut tm = std::mem::MaybeUninit::<Tm>::zeroed();
    // SAFETY: `localtime_r` writes the broken-down time into `tm`, a `struct tm` with the
    // layout of glibc, musl and the BSDs, and does not keep either pointer.
    let found = unsafe { localtime_r(&time, tm.as_mut_ptr()) };
    if found.is_null() {
        return 0;
    }
    // SAFETY: `localtime_r` succeeded, so it filled `tm`.
    unsafe { tm.assume_init() }.gmtoff
}

/// Elsewhere, the local time is taken as UTC.
#[cfg(not(all(unix, target_pointer_width = "64")))]
fn local_offset(_time: i64) -> i64 {
    0
}

/// Whether the result of a native function on Floats deserves the alert of a special
/// value (§8.2): it is NaN or infinite while its arguments were not.
pub fn made_special_float(native: Native, args: &[Value], result: &Value) -> Option<f64> {
    let Value::Float(value) = result else { return None };
    let math = matches!(
        native,
        Native::MathSqrt
            | Native::MathSin
            | Native::MathCos
            | Native::MathTan
            | Native::MathAsin
            | Native::MathAcos
            | Native::MathAtan
            | Native::MathAtan2
            | Native::MathExp
            | Native::MathLog
            | Native::MathLog10
    );
    let finite_args = args.iter().all(|arg| !matches!(arg, Value::Float(value) if !value.is_finite()));
    (math && finite_args && !value.is_finite()).then_some(*value)
}

fn new_text(text: String) -> Value {
    Value::Text(Arc::new(text))
}

/// The reason of a failed file operation, in words.
fn describe(problem: &std::io::Error) -> String {
    match problem.kind() {
        std::io::ErrorKind::NotFound => "the file does not exist".to_string(),
        std::io::ErrorKind::PermissionDenied => "permission denied".to_string(),
        std::io::ErrorKind::InvalidData => "the file is not UTF-8 text".to_string(),
        _ => problem.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waiting_needs_a_duration_from_zero_up() {
        for seconds in [-1.0, f64::NAN, f64::INFINITY] {
            let result = call(Native::TimeSleep, &[Value::Float(seconds)]);
            assert!(matches!(result, Err(BugKind::InvalidArgument { .. })), "{seconds}");
        }
        assert!(matches!(call(Native::TimeSleep, &[Value::Float(0.0)]), Ok(Value::None)));
    }

    #[test]
    fn the_local_day_is_near_the_utc_day() {
        let offset = local_offset(unix_seconds() as i64);
        assert!(offset.abs() <= 14 * 3600, "{offset}");
        let Ok(Value::Int(day)) = call(Native::DatesLocalDay, &[]) else { panic!("an Int") };
        let utc = (unix_seconds() as i64).div_euclid(86_400);
        assert!((day - utc).abs() <= 1);
    }
}
