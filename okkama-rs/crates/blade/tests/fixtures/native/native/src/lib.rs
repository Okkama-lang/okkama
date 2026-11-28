use okkama_ext::{extension, native};

#[native]
fn answer() -> i64 { 42 }

extension!("bird/native", [answer], states: []);
