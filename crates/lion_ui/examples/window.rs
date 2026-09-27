//! Opens a window, draws in it, and closes it after two seconds, or when it is closed:
//! `cargo run -p lion_ui --example window`.

fn main() -> Result<(), String> {
    let window = lion_ui::open("Lion — essai", 360, 160)?;
    let started = std::time::Instant::now();
    loop {
        lion_ui::clear(window, 0xFF_FF_FF);
        lion_ui::text(window, 16, 16, "Carnet de notes", 24, true, 0x20_20_20);
        lion_ui::text(window, 16, 52, "Léa : 14 · π ≈ 3.14159 …", 18, false, 0x20_20_20);
        lion_ui::fill(window, 16, 90, 100, 30, 0xE8_E8_E8);
        lion_ui::frame(window, 16, 90, 100, 30, 0x9A_9A_9A);
        lion_ui::text(window, 28, 96, "Charger", 18, false, 0x20_20_20);
        lion_ui::present(window)?;
        let left = 2000u64.saturating_sub(started.elapsed().as_millis() as u64);
        match lion_ui::next_event(window, Some(left.max(1)))? {
            lion_ui::Event::Close => break,
            lion_ui::Event::Timeout if left == 0 => break,
            lion_ui::Event::Timeout => {
                if started.elapsed().as_millis() >= 2000 {
                    break;
                }
            }
            event => println!("{event:?}"),
        }
    }
    lion_ui::close(window);
    Ok(())
}
