use swlc::{activate, list_surfaces};

fn main() {
    let args: Vec<String> = std::env::args().collect();

    match args.get(1).map(String::as_str) {
        Some("activate") => {
            let target = args.get(2).map(String::as_str).unwrap_or_else(|| {
                eprintln!("usage: swlc activate <app_id|title>");
                std::process::exit(1);
            });
            activate(target).unwrap_or_else(|e| {
                eprintln!("{}", e);
                std::process::exit(1);
            });
        }
        _ => {
            let surfaces = list_surfaces().unwrap_or_else(|e| {
                eprintln!("{}", e);
                std::process::exit(1);
            });

            if surfaces.is_empty() {
                println!("(no surfaces)");
                return;
            }

            for s in &surfaces {
                let state = if s.state.is_empty() {
                    String::new()
                } else {
                    format!("[{}]", s.state.join(","))
                };
                let geometry = if s.output_w > 0 {
                    format!("{}x{}+{},{}", s.output_w, s.output_h, s.output_x, s.output_y)
                } else {
                    String::new()
                };
                println!(
                    "{:<30}  {:<40}  {:<14}  {:<20}  {}",
                    if s.app_id.is_empty()  { "(unknown)"  } else { s.app_id.as_str() },
                    if s.title.is_empty()   { "(untitled)" } else { s.title.as_str()  },
                    if s.output_name.is_empty() { "-" } else { s.output_name.as_str() },
                    geometry,
                    state,
                );
            }
        }
    }
}
