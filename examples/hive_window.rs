use hive::hive::Hive;
use hive::hive_gui::{HiveWindow, Gui};
slint::include_modules!();

fn main(){
    let mut h = Hive::new("examples/properties.toml");
    HiveWindow::launch(Some(h));
    // HiveWindow::launch(None);
}