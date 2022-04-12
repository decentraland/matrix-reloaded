use indicatif::{ProgressBar, ProgressStyle};
use lipsum::lipsum;
use rand::Rng;

pub fn get_random_string() -> String {
    let random_number: usize = rand::thread_rng().gen_range(5..15);
    lipsum(random_number)
}

pub fn create_progress_bar(text: String, size: u64) -> ProgressBar {
    let progress_style = ProgressStyle::default_bar()
        .template("{prefix:>12.cyan.bold}: [{bar:57}] {pos}/{len}")
        .progress_chars("=> ");
    let progress_bar = ProgressBar::new(size);
    progress_bar.set_style(progress_style);
    progress_bar.set_prefix(text);
    progress_bar
}
