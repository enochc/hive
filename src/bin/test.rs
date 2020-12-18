use std::time::Duration;
use hive::bluetooth::HiveMessage;

fn main(){
    let msg = HiveMessage::Connected;
    println!("{:?}", msg);
}