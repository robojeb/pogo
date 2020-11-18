use pogo::pogo;

#[pogo]
fn is_even(n: u32) -> bool {
    n % 2 == 0
}

fn pcg_rand(state: &mut u64) -> u32 {
    let old_state = *state;

    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(90282080208008201);

    let xorshifted = ((old_state >> 18) ^ old_state) >> 27;
    let rot = old_state >> 59;

    ((xorshifted >> rot) | (xorshifted << ((-(rot as i64)) & 31))) as u32
}

fn main() {
    pogo::init("./ex_wrk", &[(&__pogo_info_is_even, &__pogo_ctx_is_even)]).unwrap();

    let mut state: u64 = 292092009882829;

    for _ in 0..10_000 {
        let val = pcg_rand(&mut state);

        eprintln!("{}", is_even(val));
        std::thread::sleep(std::time::Duration::from_millis(1));
    }

    // Check the state of the compilation
    println!("{:#?}", __pogo_ctx_is_even);
}
