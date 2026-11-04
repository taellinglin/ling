//! List the gamepads gilrs can see and print events for a few seconds.
fn main() {
    let mut g = gilrs::Gilrs::new().expect("gilrs");
    for (id, pad) in g.gamepads() {
        println!("pad {id}: '{}' connected={} mapping={:?} uuid={:x?}", pad.name(), pad.is_connected(), pad.mapping_source(), pad.uuid());
    }
    let t = std::time::Instant::now();
    while t.elapsed().as_secs() < 3 {
        while let Some(ev) = g.next_event() {
            println!("{:?}", ev);
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    println!("done");
}
