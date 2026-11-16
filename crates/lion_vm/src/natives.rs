//! The functions of the standard modules that the implementation provides (§23): the
//! computations come from `lion_runtime`; this file turns values into arguments and
//! results.

use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use lion_ir::Native;
use lion_runtime::BugKind;
use lion_runtime::json::{self, Json};
use lion_runtime::regex::Regex;
use lion_runtime::{codec, html, net, stdlib};

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
            let files = pairs_of(&text_list(&args[4]));
            let Value::Bool(session) = args[6] else { panic!("net.fetch expected a Bool") };
            let jar = session.then(|| text(7));
            match net::fetch(text(0), text(1), &headers, text(3), &files, float(5), jar) {
                Ok((response, jar)) => {
                    let mut words = vec![response.status.to_string(), response.body, jar.unwrap_or_default()];
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
                let first = texts(vec![
                    request.connection.to_string(),
                    request.method,
                    request.path,
                    request.query,
                    request.body,
                ]);
                let mut headers = Vec::new();
                flatten(request.headers, &mut headers);
                let mut uploads = Vec::new();
                for upload in request.uploads {
                    uploads.extend([
                        upload.field,
                        upload.filename,
                        upload.content_type,
                        upload.path,
                        upload.size.to_string(),
                    ]);
                }
                Value::List(Arc::new(vec![first, texts(headers), texts(uploads)]))
            }
            Err(message) => error(message),
        },
        Native::NetReply => {
            let headers = pairs_of(&text_list(&args[2]));
            match net::respond(int(0), int(1), &headers, text(3), text(4)) {
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
        Native::FilesWalk => {
            let mut found = Vec::new();
            match walk(std::path::Path::new(text(0)), "", &mut found) {
                Ok(()) => {
                    found.sort();
                    texts(found)
                }
                Err(problem) if problem.kind() == std::io::ErrorKind::NotFound => {
                    error(format!("cannot list `{}`: the folder does not exist", text(0)))
                }
                Err(problem) => error(format!("cannot list `{}`: {}", text(0), describe(&problem))),
            }
        }
        Native::TextSorted => {
            let mut words = text_list(&args[0]);
            words.sort_by(|a, b| stdlib::text_order(a, b));
            texts(words)
        }
        Native::TextCompare => Value::Int(match stdlib::text_order(text(0), text(1)) {
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => 1,
        }),
        Native::TextRepeat => {
            let count = int(1);
            let size = usize::try_from(count).ok().and_then(|n| n.checked_mul(text(0).len()));
            match size {
                Some(size) if size <= 1 << 30 => new_text(text(0).repeat(count as usize)),
                _ => {
                    return Err(BugKind::InvalidArgument {
                        message: "cannot repeat a text this many times".to_string(),
                        details: format!("{count} is not a count from 0 up, or the text would be too long"),
                    });
                }
            }
        }
        Native::TextFixed | Native::MathRoundTo => {
            let (x, digits) = (float(0), int(1));
            if !(0..=17).contains(&digits) {
                return Err(BugKind::InvalidArgument {
                    message: "cannot keep this many decimals".to_string(),
                    details: format!("{digits} is not a number of decimals from 0 to 17"),
                });
            }
            let written = format!("{x:.*}", digits as usize);
            if native == Native::TextFixed {
                new_text(written)
            } else {
                Value::Float(written.parse().unwrap_or(x))
            }
        }
        Native::FilesGlob => Value::Bool(stdlib::glob_match(text(0), text(1))),
        Native::FilesSize | Native::FilesModified => match std::fs::metadata(text(0)) {
            Ok(meta) if native == Native::FilesSize => Value::Int(meta.len() as i64),
            Ok(meta) => match meta.modified().map(|time| time.duration_since(UNIX_EPOCH)) {
                Ok(Ok(elapsed)) => Value::Float(elapsed.as_secs_f64()),
                Ok(Err(before)) => Value::Float(-before.duration().as_secs_f64()),
                Err(problem) => error(format!("cannot read the date of `{}`: {problem}", text(0))),
            },
            Err(problem) => error(format!("cannot read `{}`: {}", text(0), describe(&problem))),
        },
        Native::TermEnabled => Value::Bool(colors()),
        Native::TermPaint => {
            if colors() {
                new_text(format!("\u{1b}[{}m{}\u{1b}[0m", int(1), text(0)))
            } else {
                new_text(text(0).to_string())
            }
        }
        Native::SystemFolder => match std::env::current_dir() {
            Ok(folder) => new_text(folder.to_string_lossy().into_owned()),
            Err(problem) => error(format!("cannot read the current folder: {problem}")),
        },
        Native::SystemWhich => match which(text(0)) {
            Some(path) => new_text(path),
            None => Value::None,
        },
        Native::RegexSearch | Native::RegexSubstitute | Native::RegexSplit => {
            let regex = pattern(text(0))?;
            let t = text(1);
            let all = |index: usize| matches!(args[index], Value::Bool(true));
            match native {
                Native::RegexSearch => {
                    let found = matches_of(&regex, t, all(2))?;
                    let piece = |range: Option<(usize, usize)>| {
                        range.map_or(String::new(), |(a, b)| t[a..b].to_string())
                    };
                    Value::List(Arc::new(
                        found
                            .into_iter()
                            .map(|m| {
                                let (start, _) = m.groups[0].expect("a match has a range");
                                let mut words = vec![(t[..start].chars().count() + 1).to_string()];
                                words.extend(m.groups.iter().map(|range| piece(*range)));
                                texts(words)
                            })
                            .collect(),
                    ))
                }
                Native::RegexSubstitute => {
                    let found = matches_of(&regex, t, all(3))?;
                    let mut out = String::new();
                    let mut last = 0;
                    for m in &found {
                        let (start, end) = m.groups[0].expect("a match has a range");
                        out.push_str(&t[last..start]);
                        out.push_str(&lion_runtime::regex::expand(&regex, t, m, text(2)));
                        last = end;
                    }
                    out.push_str(&t[last..]);
                    new_text(out)
                }
                _ => {
                    let mut parts = Vec::new();
                    let mut last = 0;
                    for m in matches_of(&regex, t, true)? {
                        let (start, end) = m.groups[0].expect("a match has a range");
                        if end == 0 || start == t.len() && end == start {
                            continue;
                        }
                        parts.push(t[last..start].to_string());
                        last = end;
                    }
                    parts.push(t[last..].to_string());
                    texts(parts)
                }
            }
        }
        Native::RegexEscape => {
            let mut out = String::new();
            for c in text(0).chars() {
                if "\\.+*?()|[]{}^$-#".contains(c) {
                    out.push('\\');
                }
                out.push(c);
            }
            new_text(out)
        }
        Native::RegexCheck => match Regex::new(text(0)) {
            Ok(_) => Value::None,
            Err(problem) => error(format!("`{}` is not a pattern: {problem}", text(0))),
        },
        Native::HtmlSelect => {
            let Value::Bool(inside) = args[2] else { panic!("html.select_in expected a Bool") };
            let found =
                html::select(text(0), text(1), inside).map_err(|problem| BugKind::InvalidArgument {
                    message: format!("`{}` is not a selector", text(1)),
                    details: problem,
                })?;
            Value::List(Arc::new(
                found
                    .into_iter()
                    .map(|e| {
                        let mut words = vec![e.tag, e.text, e.inner, e.outer];
                        flatten(e.attributes, &mut words);
                        texts(words)
                    })
                    .collect(),
            ))
        }
        Native::HtmlText => new_text(html::text(text(0))),
        Native::HtmlEscape => new_text(html::escape(text(0))),
        Native::HtmlUnescape => new_text(html::unescape(text(0))),
        Native::UrlParts => match codec::parse_url(text(0)) {
            Ok(u) => texts(vec![u.scheme, u.user, u.host, u.port, u.path, u.query, u.fragment]),
            Err(message) => error(message),
        },
        Native::UrlJoin => match codec::join_url(text(0), text(1)) {
            Ok(joined) => new_text(joined),
            Err(message) => error(message),
        },
        Native::UrlEncode => new_text(net::encode(text(0))),
        Native::UrlDecode => match net::decode(text(0), true) {
            Some(decoded) => new_text(decoded),
            None => error(format!("`{}` decoded is not UTF-8 text", text(0))),
        },
        Native::UrlPairs => {
            let mut words = Vec::new();
            flatten(net::form_pairs(text(0)), &mut words);
            texts(words)
        }
        Native::Base64Encode => {
            let Value::Bool(url) = args[1] else { panic!("base64.encode_bytes expected a Bool") };
            new_text(codec::base64_encode(text(0).as_bytes(), url))
        }
        Native::Base64Decode => match codec::base64_decode(text(0)) {
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(decoded) => new_text(decoded),
                Err(_) => error("the bytes written in Base64 are not UTF-8 text".to_string()),
            },
            Err(message) => error(message),
        },
        Native::HashSha256 => new_text(codec::hex(&codec::sha256(text(0).as_bytes()))),
        Native::HashMd5 => new_text(codec::hex(&codec::md5(text(0).as_bytes()))),
        Native::HashHmac => new_text(codec::hex(&codec::hmac_sha256(text(0).as_bytes(), text(1).as_bytes()))),
        Native::HashFile => match std::fs::read(text(0)) {
            Ok(bytes) => new_text(codec::hex(&codec::sha256(&bytes))),
            Err(problem) => error(format!("cannot read `{}`: {}", text(0), describe(&problem))),
        },
        Native::JsonCheck => match json::parse(text(0)) {
            Ok(value) => new_text(value.to_string()),
            Err(message) => error(message),
        },
        Native::JsonOf => new_text(to_json(&args[0])?.to_string()),
        // `at `name`: …` with the step before: `at `items[2].name`: …` (C107).
        Native::JsonNest => {
            let (step, message) = (text(0), text(1));
            let joined = |inner: &str| {
                if step.starts_with('[') || inner.starts_with('[') {
                    format!("{step}{inner}")
                } else {
                    format!("{step}.{inner}")
                }
            };
            new_text(match message.strip_prefix("at `").and_then(|rest| rest.split_once("`: ")) {
                Some((inner, rest)) => format!("at `{}`: {rest}", joined(inner)),
                None => format!("at `{step}`: {message}"),
            })
        }
        Native::JsonPretty => new_text(json::cached(text(0), Json::pretty)),
        Native::JsonKind => new_text(json::cached(text(0), |v| v.kind().to_string())),
        Native::JsonMember => json_part(json::cached(text(0), |v| v.member(text(1)).map(Json::to_string))),
        Native::JsonElement => json_part(json::cached(text(0), |v| {
            usize::try_from(int(1))
                .map_err(|_| format!("there is no element {}", int(1)))
                .and_then(|i| v.element(i).map(Json::to_string))
        })),
        Native::JsonFollow => json_part(json::cached(text(0), |v| v.follow(text(1)).map(Json::to_string))),
        Native::JsonParts => texts(json::cached(text(0), |v| match v {
            Json::List(items) => items.iter().map(Json::to_string).collect(),
            Json::Object(members) => members.iter().map(|(_, value)| value.to_string()).collect(),
            _ => Vec::new(),
        })),
        Native::JsonNames => texts(json::cached(text(0), |v| match v {
            Json::Object(members) => members.iter().map(|(name, _)| name.clone()).collect(),
            _ => Vec::new(),
        })),
        Native::JsonScalar => new_text(json::cached(text(0), |v| match v {
            Json::Text(t) => t.clone(),
            Json::Number(n) => n.clone(),
            other => other.to_string(),
        })),
        Native::JsonWith | Native::JsonPlus => {
            let mut value = json::cached(text(0), Json::clone);
            let added = json::cached(text(if native == Native::JsonWith { 2 } else { 1 }), Json::clone);
            match (&mut value, native) {
                (Json::Object(members), Native::JsonWith) => {
                    match members.iter_mut().find(|(name, _)| name == text(1)) {
                        Some((_, old)) => *old = added,
                        None => members.push((text(1).to_string(), added)),
                    }
                }
                (Json::List(items), Native::JsonPlus) => items.push(added),
                (other, _) => {
                    let (wanted, call) =
                        if native == Native::JsonWith { ("an object", "with") } else { ("a list", "plus") };
                    return Err(BugKind::InvalidArgument {
                        message: format!("`v.{call}(…)` needs {wanted}"),
                        details: format!("the value is a {}", other.kind()),
                    });
                }
            }
            new_text(value.to_string())
        }
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

/// A pattern of the module `regex`; one that is not a pattern is a bug.
fn pattern(source: &str) -> Result<Regex, BugKind> {
    Regex::new(source).map_err(|problem| BugKind::InvalidArgument {
        message: format!("`{source}` is not a pattern"),
        details: problem,
    })
}

/// The files inside `folder`, as paths from the first folder, `/` between their parts.
/// A link to a folder is not followed, so that a loop of links ends.
fn walk(folder: &std::path::Path, prefix: &str, found: &mut Vec<String>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(folder)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let relative = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
        let kind = entry.file_type()?;
        if kind.is_dir() {
            walk(&entry.path(), &relative, found)?;
        } else if kind.is_file() || entry.path().is_file() {
            found.push(relative);
        }
    }
    Ok(())
}

/// Whether the output takes colors: a terminal, without `NO_COLOR`, and not `TERM=dumb`.
fn colors() -> bool {
    use std::io::IsTerminal;
    std::io::stdout().is_terminal()
        && std::env::var_os("NO_COLOR").is_none_or(|value| value.is_empty())
        && std::env::var("TERM").is_ok_and(|term| term != "dumb")
}

/// The program that a command line would run under this name, found in `PATH`.
fn which(program: &str) -> Option<String> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|folder| folder.join(program))
        .find(|candidate| executable(candidate))
        .map(|path| path.to_string_lossy().into_owned())
}

#[cfg(unix)]
fn executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata().is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn executable(path: &std::path::Path) -> bool {
    path.is_file() || path.with_extension("exe").is_file()
}

/// The first match, or every match; a search that takes too many steps is a bug.
fn matches_of(regex: &Regex, t: &str, all: bool) -> Result<Vec<lion_runtime::regex::Found>, BugKind> {
    let found =
        if all { regex.find_all(t) } else { regex.find_at(t, 0).map(|found| found.into_iter().collect()) };
    found.map_err(|problem| BugKind::InvalidArgument {
        message: "the search did not finish".to_string(),
        details: problem,
    })
}

/// A part of a JSON value, or an Error.
fn json_part(part: Result<String, String>) -> Value {
    match part {
        Ok(part) => new_text(part),
        Err(message) => Value::Error(Arc::new(message)),
    }
}

/// The JSON form of a value of Lion (`json.of`, C105).
fn to_json(value: &Value) -> Result<Json, BugKind> {
    let all = |values: &[Value]| values.iter().map(to_json).collect::<Result<Vec<_>, _>>();
    let number =
        |x: f64| if x.is_finite() { Json::Number(lion_runtime::format::format_float(x)) } else { Json::Null };
    Ok(match value {
        Value::None => Json::Null,
        Value::Bool(b) => Json::Bool(*b),
        Value::Int(n) => Json::int(*n),
        Value::Float(x) => number(*x),
        Value::Rational(r) => number(r[0] as f64 / r[1] as f64),
        Value::Text(t) => Json::text(t.as_str()),
        Value::Error(message) => Json::object([("error", Json::text(message.as_str()))]),
        Value::List(items) | Value::Tuple(items) => Json::List(all(items)?),
        Value::Range(bounds) => Json::List((bounds[0]..=bounds[1]).map(Json::int).collect()),
        Value::Set(set) => Json::List(all(set.items())?),
        Value::Map(map) => Json::Object(
            map.entries()
                .iter()
                .map(|(key, value)| Ok((key.to_text(), to_json(value)?)))
                .collect::<Result<_, BugKind>>()?,
        ),
        // A `json.Value` is already JSON.
        Value::Struct(record) if record.layout.name == "json.Value" && record.layout.fields.len() == 1 => {
            json::parse(&record.fields[0].to_text()).unwrap_or(Json::Null)
        }
        Value::Struct(record) => Json::Object(
            record
                .layout
                .fields
                .iter()
                .zip(&record.fields)
                .map(|(name, value)| Ok((name.clone(), to_json(value)?)))
                .collect::<Result<_, BugKind>>()?,
        ),
        Value::Enum(enumeration, position) => Json::text(enumeration.values[*position as usize].as_str()),
        Value::Cell(cell) => to_json(&cell.lock().unwrap_or_else(|poisoned| poisoned.into_inner()))?,
        Value::Task(_) | Value::Function(_) | Value::Ref(_) => {
            return Err(BugKind::InvalidArgument {
                message: "this value has no JSON form".to_string(),
                details: format!("a {} is neither data nor text", value.type_name()),
            });
        }
    })
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
