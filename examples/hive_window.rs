use std::rc::{Rc};
use std::sync::{Arc, Weak, Mutex};
use log::{debug, LevelFilter};
use hive::hive::Hive;
use hive::hive_gui::{HiveWindow, Gui};
use hive::init_logging;
use hive::property::Property;
slint::include_modules!();

fn main(){
    init_logging(Some(LevelFilter::Debug));


    let mut h = Hive::new("examples/listen_3000.toml");

    HiveWindow::launch(Some(h));
}
