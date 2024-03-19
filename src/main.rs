
mod hive_gui;

use std::{env};
use crate::hive_gui::HiveWindow;


fn main() {

    let g: HiveWindow = HiveWindow{};
    use hive_gui::Gui;
    g.launch(None);
    // let args: Vec<String> = env::args().collect();
    //
    // for (i, name) in args.iter().enumerate() {
    //     println!("{} {:?}", name, i);
    //     if name == "connect" {
    //         let list_val = args.get(i + 1);
    //         if list_val.is_some() {
    //             println!("connect :: {:?}", list_val);
    //         }
    //     }
    // }
}
