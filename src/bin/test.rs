use std::time::Duration;

fn main(){
    async_std::task::spawn(async move{

        println!("111");
        // async_std::task::sleep(Duration::from_secs(2)).await;
        std::thread::sleep(Duration::from_secs(2));
        println!("<< STOPPING 11");

    });
    async_std::task::block_on(async move{

        println!("222");
        async_std::task::sleep(Duration::from_secs(2)).await;
        // std::thread::sleep(Duration::from_secs(2));
        println!("<< STOPPING 22");

    });
    println!("<< STOPPed");
}