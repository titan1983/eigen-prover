use crate::contexts::{AggContext, BatchContext, FinalContext};
use crate::provers::{AggProver, BatchProver, FinalProver, Prover};
use crate::stage::Stage;

use anyhow::{anyhow, bail, Result};
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::Mutex;
use uuid::Uuid;

pub struct Pipeline {
    basedir: String,
    queue: VecDeque<String>, // task_id
    task_map: Mutex<HashMap<String, Stage>>,
    task_name: String,
}

impl Pipeline {
    pub fn new(basedir: String, task_name: String) -> Self {
        // TODO: recover tasks from basedir
        Pipeline {
            basedir,
            queue: VecDeque::new(),
            task_map: Mutex::new(HashMap::new()),
            task_name,
        }
    }

    pub fn get_key(&self, task_id: &String, chunk_id: &String) -> String {
        format!("{}_{}", task_id, chunk_id)
    }

    fn save_checkpoint(&self, key: &String, finished: bool) -> Result<String> {
        let binding = self.task_map.lock().unwrap();
        let task = binding.get(key);

        if let Some(status) = task {
            // mkdir
            let workdir = Path::new(&self.basedir).join(status.path());
            log::info!("save_checkpoint, mkdir: {:?}", workdir);
            let _ = std::fs::create_dir_all(workdir.clone());

            if !finished {
                let p = workdir.join("status");
                std::fs::write(p, status.to_string()?)?;
            }

            let p = workdir.join("status.finished");
            std::fs::write(p, if finished { "1" } else { "0" })?;
        }
        Ok(key.clone())
    }

    fn load_checkpoint(&self, key: &String) -> Result<bool> {
        let p = Path::new(&self.basedir)
            .join("proof")
            .join(key)
            .join("status.finished");
        let status: bool = std::fs::read_to_string(p)?.parse().map_err(|e| {
            log::error!("load_checkpoint");
            anyhow!("load checkpoint failed, {:?}", e)
        })?;
        Ok(status)
    }

    pub fn batch_prove(&mut self, task_id: String, chunk_id: String) -> Result<String> {
        let key = self.get_key(&task_id, &chunk_id);
        match self.task_map.get_mut() {
            Ok(w) => {
                self.queue.push_back(key.clone());
                w.insert(key.clone(), Stage::Batch(task_id.clone(), chunk_id));
                self.save_checkpoint(&key, false)
            }
            _ => bail!("Task queue is full".to_string()),
        }
    }

    /// Add a new task into task queue
    pub fn aggregate_prove(&mut self, task: String, task2: String) -> Result<String> {
        let task_id = Uuid::new_v4().to_string();
        let key = self.get_key(&task_id, &"agg".to_string());
        match self.task_map.get_mut() {
            Ok(w) => {
                self.queue.push_back(key.clone());
                w.insert(key.clone(), Stage::Aggregate(key.clone(), task, task2));
                self.save_checkpoint(&key, false)?;
                Ok(task_id)
            }
            _ => bail!("Task queue is full".to_string()),
        }
    }

    /// Add a new task into task queue
    pub fn final_prove(
        &mut self,
        task_id: String,
        curve_name: String,
        prover_addr: String,
    ) -> Result<String> {
        let key = self.get_key(&task_id, &"final".to_string());
        match self.task_map.get_mut() {
            Ok(w) => {
                self.queue.push_back(key.clone());
                w.insert(
                    key.clone(),
                    Stage::Final(task_id.clone(), curve_name, prover_addr), // use task_id first, then compute the right task_name in final context
                );
                self.save_checkpoint(&key, false)?;
                Ok(task_id)
            }
            _ => bail!("Task queue is full".to_string()),
        }
    }

    pub fn cancel(&mut self, task_id: String) -> Result<()> {
        // TODO find all the tasks with prefix `task_id`
        if let Ok(w) = self.task_map.get_mut() {
            let _ = w.remove(&task_id);
        }
        Ok(())
    }

    /// Return prover status
    pub fn get_status(&mut self) -> Result<()> {
        Ok(())
    }

    pub fn get_proof(&mut self, key: String, _timeout: u64) -> Result<String> {
        match self.load_checkpoint(&key) {
            Ok(true) => Ok(key),
            Ok(false) => bail!(format!("can not find task: {}", key)),
            Err(e) => bail!(format!("load checkpoint failed, {:?}", e)),
        }
    }

    pub fn prove(&mut self) -> Result<()> {
        if let Some(key) = self.queue.pop_front() {
            match self.task_map.get_mut().unwrap().get(&key) {
                Some(v) => match v {
                    Stage::Batch(task_id, chunk_id) => {
                        let ctx = BatchContext::new(
                            &self.basedir,
                            task_id,
                            &self.task_name.clone(),
                            chunk_id,
                        );
                        BatchProver::new().prove(&ctx)?;
                    }
                    Stage::Aggregate(task_id, input, input2) => {
                        let ctx = AggContext::new(
                            &self.basedir,
                            task_id,
                            &self.task_name,
                            input.clone(),
                            input2.clone(),
                        );
                        AggProver::new().prove(&ctx)?;
                    }
                    Stage::Final(task_id, curve_name, prover_addr) => {
                        let ctx = FinalContext::new(
                            self.basedir.clone(),
                            task_id.clone(),
                            self.task_name.clone(),
                            curve_name.clone(),
                            prover_addr.clone(),
                        );
                        FinalProver::new().prove(&ctx)?;
                    }
                },
                _ => {
                    log::info!("Task queue is empty...");
                }
            };
            self.save_checkpoint(&key, true)?;
        }
        Ok(())
    }
}
