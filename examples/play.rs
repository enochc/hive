#![allow(warnings)]
use async_std::{
    prelude::*,
    task,
};
use futures::channel::mpsc;
use futures::SinkExt;
use async_std::sync::{RwLock, Arc};


pub struct Signal<T>
where T: Clone + 'static
 {
    slots: Vec<Arc<Box<dyn Fn(T)>>>,
}

impl<T> Clone for Signal<T>
    where T: Clone{
    fn clone(&self) -> Signal<T>{
        self.slots.clone();
        return Signal{
            slots: self.slots.clone()
        }
    }
}

impl<T> Signal<T>
    where T: Clone {
    fn new(func: Box<dyn Fn(T)>) -> Signal<T> {
        //let mut v = Vec::new();
        //v.push(Arc::new(func));
        let v = vec![Arc::new(func)];
        return Signal{
            slots: v
        }
    }
    fn connect(mut self, func:fn(T)) {
        self.slots.push(Arc::new(Box::new(func)));
    }
}

fn main(){
    // task::block_on(run());
    fn bb(it:u32) {
        println!("<<<<< do a thing {}", it);
    }
    let s:Signal<u32> = Signal::new(Box::new(bb));
    let p = s.clone();


    // let b = bb;
    // let c = b.clone();
    // c(4);
    // s();
}

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
