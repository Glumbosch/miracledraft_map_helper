use std::{
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

fn main() {
    let build_time = Command::new("date")
        .args(["+%Y-%m-%d %H:%M:%S %Z"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|value| value.trim().to_owned())
        .unwrap_or_else(|| {
            format!(
                "Unix {}",
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|duration| duration.as_secs())
                    .unwrap_or(0)
            )
        });
    println!("cargo:rustc-env=MIRACLEDRAFT_BUILD_TIME={build_time}");
}
