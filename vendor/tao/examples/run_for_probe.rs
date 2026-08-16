use std::{
  thread,
  time::{Duration, Instant},
};
use tao::event_loop::{ControlFlow, EventLoopBuilder};

fn run_for(event_loop: &mut tao::event_loop::EventLoop<()>, timeout: Duration) -> Duration {
  let start = Instant::now();
  event_loop.run_for(timeout, |_, _, control_flow| {
    *control_flow = ControlFlow::Wait;
  });
  start.elapsed()
}

fn main() {
  let mut event_loop = EventLoopBuilder::<()>::with_user_event().build();

  for i in 1..=3 {
    let elapsed = run_for(&mut event_loop, Duration::from_millis(50));
    println!("timeout {i}: {} ms", elapsed.as_millis());
    assert!(elapsed >= Duration::from_millis(35));
    assert!(elapsed < Duration::from_millis(500));
  }

  let proxy = event_loop.create_proxy();
  thread::spawn(move || {
    thread::sleep(Duration::from_millis(25));
    proxy.send_event(()).unwrap();
  });

  let elapsed = run_for(&mut event_loop, Duration::from_millis(500));
  println!("native wake: {} ms", elapsed.as_millis());
  assert!(elapsed >= Duration::from_millis(10));
  assert!(elapsed < Duration::from_millis(300));

  let elapsed = run_for(&mut event_loop, Duration::from_millis(50));
  println!("post-wake timeout: {} ms", elapsed.as_millis());
  assert!(elapsed >= Duration::from_millis(35));
  assert!(elapsed < Duration::from_millis(500));
}
