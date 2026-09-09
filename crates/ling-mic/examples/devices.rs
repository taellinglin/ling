use cpal::traits::{DeviceTrait, HostTrait};

fn main() {
    let host = cpal::default_host();
    println!("host: {:?}", host.id());

    match host.default_input_device() {
        Some(d) => println!("default input: {:?}", d.name()),
        None => println!("default input: NONE"),
    }

    println!("-- all input devices --");
    match host.input_devices() {
        Ok(devices) => {
            for (i, d) in devices.enumerate() {
                let name = d.name().unwrap_or_else(|_| "<unnamed>".into());
                let cfg = d.default_input_config();
                println!("[{i}] {name}  default_config={cfg:?}");
                if let Ok(supported) = d.supported_input_configs() {
                    for sc in supported {
                        println!("      supports: {sc:?}");
                    }
                }
            }
        },
        Err(e) => println!("input_devices() failed: {e:?}"),
    }
}
