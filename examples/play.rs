use std::num;

fn main (){
    // let bignum: u32 = u32::max_value();
    // println!("<<<< {:?}", bignum);
    // println!("<<<< {:?}", bignum.to_be_bytes());
    // println!("<<<< {:?}", bignum as u32);
    // println!("<<<< {:?}", bignum as u8);
    let mut x: Option<f32> = None;
// ...

    x = Some(3.5);
// ...

    if let Some(v) = x {
        println!("x has value: {}", v);
    }
    else {
        println!("x is not set");
    }
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