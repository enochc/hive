use std::{env};

// TODO this is work in progress
fn main() {
    let args: Vec<String> = env::args().collect();

    for (i, name) in args.iter().enumerate() {
        println!("{} {:?}", name, i);
        if name == "connect" {
            let list_val = args.get(i + 1);
            if list_val.is_some() {
                println!("connect :: {:?}", list_val);
            }
        }
    }
}
