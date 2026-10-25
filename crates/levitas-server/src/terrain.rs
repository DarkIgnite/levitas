//! Where new chunks come from, and generating them away from the world thread.
//!
//! Generating real terrain takes milliseconds per chunk, far too long to do on the tick
//! thread for the hundreds of chunks a joining player needs. A pool of worker threads
//! generates the areas the world asks for and hands their chunks back through a channel.

use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use levitas_pumpkin::PumpkinTerrain;
use levitas_world::Chunk;
use tokio::sync::mpsc as tokio_mpsc;

/// A generator of chunks.
pub trait Terrain: Send + Sync {
    fn generate(&self, x: i32, z: i32) -> Chunk;

    /// Chunks per side of the square areas generated together: trees and structures
    /// cross chunk borders, so real terrain comes out right only a whole area at a time.
    fn area(&self) -> i32 {
        1
    }

    /// Every chunk of area (`ax`, `az`), the one holding chunks
    /// `ax * area..(ax + 1) * area` along x and likewise along z.
    fn generate_area(&self, ax: i32, az: i32) -> Vec<Generated> {
        vec![((ax, az), self.generate(ax, az))]
    }

    /// Where players spawn in a new world: a block position to stand at.
    fn spawn_point(&self) -> [i32; 3];

    /// Whether generating is so cheap that chunks are made on the spot, and not worth
    /// saving until something changes in them.
    fn is_cheap(&self) -> bool {
        false
    }
}

impl Terrain for PumpkinTerrain {
    fn generate(&self, x: i32, z: i32) -> Chunk {
        PumpkinTerrain::generate(self, x, z)
    }

    fn spawn_point(&self) -> [i32; 3] {
        PumpkinTerrain::spawn_point(self)
    }

    fn area(&self) -> i32 {
        levitas_pumpkin::AREA
    }

    fn generate_area(&self, ax: i32, az: i32) -> Vec<Generated> {
        PumpkinTerrain::generate_area(self, ax, az)
    }
}

/// A finished chunk: its position and contents.
pub type Generated = ((i32, i32), Chunk);

/// Worker threads that generate areas on request.
pub struct GenerationPool {
    jobs: mpsc::Sender<(i32, i32)>,
}

impl GenerationPool {
    /// Starts `workers` threads; finished areas arrive on the returned receiver.
    pub fn start(
        terrain: Arc<dyn Terrain>,
        workers: usize,
    ) -> (Self, tokio_mpsc::UnboundedReceiver<Vec<Generated>>) {
        let (jobs, queue) = mpsc::channel::<(i32, i32)>();
        let queue = Arc::new(Mutex::new(queue));
        let (done, finished) = tokio_mpsc::unbounded_channel();
        for i in 0..workers.max(1) {
            let queue = Arc::clone(&queue);
            let done = done.clone();
            let terrain = Arc::clone(&terrain);
            std::thread::Builder::new()
                .name(format!("levitas-gen-{i}"))
                .spawn(move || loop {
                    let job = match queue.lock() {
                        Ok(queue) => queue.recv(),
                        Err(_) => return,
                    };
                    let Ok((x, z)) = job else {
                        return; // the world is gone
                    };
                    if done.send(terrain.generate_area(x, z)).is_err() {
                        return;
                    }
                })
                .expect("generation thread starts");
        }
        (GenerationPool { jobs }, finished)
    }

    /// Asks for area (`ax`, `az`).
    pub fn request(&self, ax: i32, az: i32) {
        // Fails only if every worker died, in which case the area never arrives and the
        // player keeps seeing the edge of the loaded world.
        let _ = self.jobs.send((ax, az));
    }
}

/// Worker threads to use: all cores but one, which is left for the world and network.
pub fn default_workers() -> usize {
    std::thread::available_parallelism().map_or(2, |n| n.get().saturating_sub(1).max(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flat::FlatGenerator;

    #[tokio::test]
    async fn pool_generates_requested_chunks() {
        let (pool, mut finished) = GenerationPool::start(Arc::new(FlatGenerator::new()), 2);
        for x in 0..4 {
            pool.request(x, -x);
        }
        let mut got: Vec<(i32, i32)> = Vec::new();
        for _ in 0..4 {
            got.extend(
                finished
                    .recv()
                    .await
                    .unwrap()
                    .into_iter()
                    .map(|(pos, _)| pos),
            );
        }
        got.sort_unstable();
        assert_eq!(got, [(0, 0), (1, -1), (2, -2), (3, -3)]);
    }
}
