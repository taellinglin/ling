//! Lists the CUDA devices with their free memory, as the "auto" device choice sees them.
fn main() {
    println!("auto picks cuda:{}", ling_diffusion::auto_device());
    use candle_core::cuda_backend::cudarc::driver::CudaContext;
    let n = CudaContext::device_count();
    println!("count {n:?}");
    for i in 0..n.unwrap_or(0) as usize {
        let c = CudaContext::new(i).map(|c| (c.name(), c.mem_get_info()));
        println!("{i}: {c:?}");
    }
}
