use lipsum::lipsum;
use rand::Rng;

pub fn get_random_string() -> String {
    let random_number: usize = rand::thread_rng().gen_range(5..15);
    lipsum(random_number)
}
