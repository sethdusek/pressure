# Pressure (name pending)

Memory pressure handling on Linux, using pressure stall information and systemd's memory pressure interface (https://systemd.io/MEMORY_PRESSURE/).

# Usage

Pressure (name pending, again) provides a **PressureMonitor** that can be used to wait pressure events. What you choose to do when encountering these events depends, some examples would be:

- Call malloc_trim (not sure what the Rust equivalent is, because the Allocator API doesn't expose it, so it'd depend on whether you're using the system allocator/jemalloc/mimalloc or something else)
- Free caches
- Terminate the process entirely if it can be restored later

```rust
use pressure::PressureMonitor;
fn main() {
    let monitor = PressureMonitor::new().unwrap();
    std::thread::spawn(move || {
        loop {
            monitor.wait().unwrap();
            // Do something when pressure is detected
        }
    })
}
```

Similarly, an async equivalent is exposed, via the *tokio* feature:

```rust
use pressure::tokio::PressureMonitor;
fn main() {
    let monitor = PressureMonitor::new().unwrap();
    tokio::spawn(async move {
        loop {
            monitor.wait().await.unwrap();
            // Do something when pressure is detected
        }
    });
}
```
