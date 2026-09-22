//! What the desktop looks like to `native-term-os`: run it on a box to
//! see which source answered (`cargo run -p native-term-os --example
//! appearance`).

fn main() {
    let look = native_term_os::appearance::read();
    println!("dark:      {:?}", look.dark);
    println!("accent:    {:?}", look.accent.map(|(r, g, b)| format!("#{r:02x}{g:02x}{b:02x}")));
    println!("monospace: {:?}", look.monospace);
    println!("source:    {:?}", look.source);
}
