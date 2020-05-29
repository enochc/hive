use async_std::{
    prelude::*,
    task,
};
use futures::channel::mpsc;
use futures::SinkExt;

fn main(){
    task::block_on(run());
}
#[allow(unused_must_use)]
async fn run (){
    let (tx,mut rx) = mpsc::channel(5);
    let mut  tx = tx.clone();

    task::spawn(async move{
        for x in 0..5 {

            tx.send(x).await;

            println!("sending: {}", x);
        }
        // tx.flush().await;

    });

    task::spawn( async move{
        loop {
            match rx.next().await {
                Some(val) => println!("received: {}", val),
                _ => println!("something else")
            }
        }
    }).await;





}

/*
  let mut x: Option<f32> = None;
// ...

    x = Some(3.5);
// ...

    if let Some(value) = x {
        println!("x has value: {}", value);
    }
    else {
        println!("x is not set");
    }
 */