
use std::sync::mpsc::{Sender, Receiver};
use std::sync::mpsc;
use std::sync::{Arc, RwLock};
use std::thread;
use futures::executor::block_on;


#[derive(Default)]
pub struct Signal<T> {
    slots: Arc<RwLock<Vec<Arc<dyn Fn(T) + Send + Sync + 'static>>>>
}

async fn sendEmit<T>(func: Arc<dyn Fn(T) + Send + Sync + 'static>, val: T)
//where T: Send + Copy + Sync +'static, // Do I need this line? I used to
{
    func(val)
}

impl<T> Signal<T> {
    #[tokio::main]
    pub async fn emit(&mut self, val: T)
    // where T: Clone + Send + Copy + 'static, // How about this?
        where T: Sync +Copy + Send +'static,
    {
        let slots_clone= self.slots.clone();
        let (tx, rx): (Sender<bool>, Receiver<bool>) = mpsc::channel();
        let mut numThreads = 0;

        // Spawn thread for each attached slot
        for s in slots_clone.read().unwrap().iter() {
            let thread_tx = tx.clone();
            let s_clone = s.clone();
            numThreads +=1;
            thread::spawn(  move|| {
                block_on(sendEmit(s_clone, val));
                thread_tx.send(true)
            });

        }

        // Wait for threads to complete
        for _ in 0..numThreads {
            rx.recv();
        }
    }

    pub fn connect(&mut self, slot: impl Fn(T) + Send + Sync + 'static) {
        self.slots.write().expect("Failed to get write lock on slots").push(Arc::new(slot));
    }
}
