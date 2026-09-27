//! The functions of the standard modules that the implementation provides (§23): the
//! computations come from `lion_runtime`; this file turns values into arguments and
//! results.

use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use lion_ir::Native;
use lion_runtime::BugKind;
use lion_runtime::stdlib;

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
