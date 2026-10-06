use win_desktop_duplication::devices::AdapterFactory;
use win_desktop_duplication::{set_process_dpi_awareness, DesktopDuplicationApi};

fn main() {
    println!("=== Testing win_desktop_duplication ===");
    set_process_dpi_awareness();

    let mut adapters: Vec<_> = AdapterFactory::new().collect();
    println!("Found {} adapters", adapters.len());
    for (i, a) in adapters.iter().enumerate() {
        println!("  Adapter #{}: {}", i, a.name());
    }

    if adapters.is_empty() {
        println!("No adapters!");
        return;
    }

    let adapter = adapters.remove(0);
    let mut displays: Vec<_> = adapter.iter_displays().collect();
    println!("Found {} displays on adapter", displays.len());
    for (i, d) in displays.iter().enumerate() {
        println!("  Display #{}: {}", i, d.name());
    }

    if displays.is_empty() {
        println!("No displays!");
        return;
    }

    let display = displays.remove(0);
    println!("Creating DesktopDuplicationApi...");
    match DesktopDuplicationApi::new(adapter, display) {
        Ok(mut dupl) => {
            println!("✅ DesktopDuplicationApi created successfully!");
            match dupl.acquire_next_frame_now() {
                Ok(_) => println!("✅ Frame acquired!"),
                Err(e) => println!("❌ Error acquiring frame: {:?}", e),
            }
        }
        Err(e) => {
            println!("❌ Failed to create DesktopDuplicationApi: {:?}", e);
        }
    }
}
