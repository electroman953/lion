//! `lion run file.lion words…`: the words after the file reach `system.args()` (C104).

use std::process::Command;

#[test]
fn the_words_after_the_file_are_the_programs() {
    let folder = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("args");
    std::fs::create_dir_all(&folder).unwrap();
    let program = folder.join("args.lion");
    std::fs::write(&program, "use system\nshow(system.args())\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_lion"))
        .arg("run")
        .arg(&program)
        .args(["a", "b c", "--flag"])
        .output()
        .expect("the lion binary runs");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "[\"a\", \"b c\", \"--flag\"]\n");
}
