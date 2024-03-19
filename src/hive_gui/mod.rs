mod build;

use std::env;
use slint;
use slint::{CloseRequestResponse, Model, VecModel, Weak};
use hive::hive::Hive;

pub trait Gui{
    fn launch(&self, hive: Option<Hive>);
}
pub struct HiveWindow{}

impl Gui for HiveWindow {
    fn launch(&self, hive: Option<Hive>) {
        let window = MainWindow::new().unwrap();

        window.window().on_close_requested(move||{
                println!("<< done");
                return CloseRequestResponse::HideWindow;

        });
        window.run().unwrap();
        // window
    }
}


slint::slint! {
    export component MainWindow inherits Window {
        min-height: 50px;
        min-width: 100px;

        preferred-height: 200px;
        preferred-width: 200px;
        // ListView {



            Text {
                text: "hello world";
                color: green;
            }
            // Button {
            //     text: "click me";
            //     // clicked => {
            //     //     println!("clicked");
            //     // }
            // }
        // }

    }
}