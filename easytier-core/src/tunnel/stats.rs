use std::{
    cell::UnsafeCell,
    sync::atomic::{AtomicU32, Ordering::Relaxed},
};

pub struct WindowLatency {
    latency_us_window: Vec<AtomicU32>,
    latency_us_window_index: AtomicU32,
    latency_us_window_size: u32,

    sum: AtomicU32,
    count: AtomicU32,
}

impl std::fmt::Debug for WindowLatency {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WindowLatency")
            .field("count", &self.count)
            .field("window_size", &self.latency_us_window_size)
            .field("window_latency", &self.get_latency_us::<u32>())
            .finish()
    }
}

impl WindowLatency {
    pub fn new(window_size: u32) -> Self {
        Self {
            latency_us_window: (0..window_size).map(|_| AtomicU32::new(0)).collect(),
            latency_us_window_index: AtomicU32::new(0),
            latency_us_window_size: window_size,

            sum: AtomicU32::new(0),
            count: AtomicU32::new(0),
        }
    }

    pub fn record_latency(&self, latency_us: u32) {
        let index = self.latency_us_window_index.fetch_add(1, Relaxed);
        if self.count.load(Relaxed) < self.latency_us_window_size {
            self.count.fetch_add(1, Relaxed);
        }

        let index = index % self.latency_us_window_size;
        let old_lat = self.latency_us_window[index as usize].swap(latency_us, Relaxed);

        if old_lat < latency_us {
            self.sum.fetch_add(latency_us - old_lat, Relaxed);
        } else {
            self.sum.fetch_sub(old_lat - latency_us, Relaxed);
        }
    }

    pub fn get_latency_us<T: From<u32> + std::ops::Div<Output = T>>(&self) -> T {
        let count = self.count.load(Relaxed);
        let sum = self.sum.load(Relaxed);
        if count == 0 {
            0.into()
        } else {
            (T::from(sum)) / T::from(count)
        }
    }

    /// Mean absolute consecutive RTT delta over the current window (microseconds).
    /// Returns 0 until at least two samples are present.
    pub fn get_jitter_us(&self) -> u64 {
        let count = self.count.load(Relaxed) as usize;
        if count < 2 {
            return 0;
        }

        let size = self.latency_us_window_size as usize;
        let next = self.latency_us_window_index.load(Relaxed) as usize;
        let mut prev: Option<u32> = None;
        let mut sum_abs_diff: u64 = 0;
        let mut pairs: u64 = 0;

        let visit = |idx: usize, prev: &mut Option<u32>, sum: &mut u64, pairs: &mut u64| {
            let cur = self.latency_us_window[idx].load(Relaxed);
            if let Some(p) = *prev {
                let a = p as u64;
                let b = cur as u64;
                *sum += a.abs_diff(b);
                *pairs += 1;
            }
            *prev = Some(cur);
        };

        if count < size {
            for i in 0..count {
                visit(i, &mut prev, &mut sum_abs_diff, &mut pairs);
            }
        } else {
            let start = next % size;
            for i in 0..size {
                visit((start + i) % size, &mut prev, &mut sum_abs_diff, &mut pairs);
            }
        }

        if pairs == 0 {
            0
        } else {
            sum_abs_diff / pairs
        }
    }
}

#[derive(Debug)]
pub struct Throughput {
    tx_bytes: UnsafeCell<u64>,
    rx_bytes: UnsafeCell<u64>,
    tx_packets: UnsafeCell<u64>,
    rx_packets: UnsafeCell<u64>,
}

impl Clone for Throughput {
    fn clone(&self) -> Self {
        Self {
            tx_bytes: UnsafeCell::new(unsafe { *self.tx_bytes.get() }),
            rx_bytes: UnsafeCell::new(unsafe { *self.rx_bytes.get() }),
            tx_packets: UnsafeCell::new(unsafe { *self.tx_packets.get() }),
            rx_packets: UnsafeCell::new(unsafe { *self.rx_packets.get() }),
        }
    }
}

unsafe impl Send for Throughput {}
unsafe impl Sync for Throughput {}

impl Default for Throughput {
    fn default() -> Self {
        Self {
            tx_bytes: UnsafeCell::new(0),
            rx_bytes: UnsafeCell::new(0),
            tx_packets: UnsafeCell::new(0),
            rx_packets: UnsafeCell::new(0),
        }
    }
}

impl Throughput {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn tx_bytes(&self) -> u64 {
        unsafe { *self.tx_bytes.get() }
    }

    pub fn rx_bytes(&self) -> u64 {
        unsafe { *self.rx_bytes.get() }
    }

    pub fn tx_packets(&self) -> u64 {
        unsafe { *self.tx_packets.get() }
    }

    pub fn rx_packets(&self) -> u64 {
        unsafe { *self.rx_packets.get() }
    }

    pub fn record_tx_bytes(&self, bytes: u64) {
        unsafe {
            *self.tx_bytes.get() += bytes;
            *self.tx_packets.get() += 1;
        }
    }

    pub fn record_rx_bytes(&self, bytes: u64) {
        unsafe {
            *self.rx_bytes.get() += bytes;
            *self.rx_packets.get() += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::WindowLatency;

    #[test]
    fn jitter_is_zero_with_fewer_than_two_samples() {
        let w = WindowLatency::new(8);
        assert_eq!(w.get_jitter_us(), 0);
        w.record_latency(1_000);
        assert_eq!(w.get_jitter_us(), 0);
    }

    #[test]
    fn jitter_is_mean_absolute_consecutive_delta() {
        let w = WindowLatency::new(8);
        // deltas: |20-10|=10, |30-20|=10 → mean 10
        w.record_latency(10);
        w.record_latency(20);
        w.record_latency(30);
        assert_eq!(w.get_jitter_us(), 10);
    }

    #[test]
    fn jitter_survives_ring_wrap() {
        let w = WindowLatency::new(3);
        w.record_latency(100);
        w.record_latency(200);
        w.record_latency(100);
        // wrap: overwrite oldest (100) with 400 → window chronological: 200, 100, 400
        w.record_latency(400);
        // deltas: 100, 300 → mean 200
        assert_eq!(w.get_jitter_us(), 200);
    }
}
