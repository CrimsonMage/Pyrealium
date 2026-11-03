//! The game server's database thread: one connection, jobs run strictly in submission order, so
//! a character's logoff save always lands before anything queued after it. The thread holds the
//! server lock (`Database::try_server_lock`) for its whole life.
use super::{Connection, Database};
use std::{
    io,
    sync::mpsc::{self, Receiver, Sender},
    thread::{self, JoinHandle},
};

type Job = Box<dyn FnOnce(&mut Connection) + Send>;

pub struct Writer {
    jobs: Option<Sender<Job>>,
    thread: Option<JoinHandle<()>>,
}

impl Writer {
    /// Connects, takes the server lock and applies pending migrations.
    pub fn start(database: &Database) -> io::Result<Self> {
        database.ensure_exists()?;
        let database = database.clone();
        let (ready_tx, ready_rx) = mpsc::channel::<io::Result<()>>();
        let (jobs, queue): (Sender<Job>, Receiver<Job>) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("pyrealium-db".into())
            .spawn(move || {
                let opened = database.try_server_lock().and_then(|lock| {
                    let lock = lock.ok_or_else(|| {
                        io::Error::new(
                            io::ErrorKind::AddrInUse,
                            format!(
                                "another Pyrealium server is already using database {}; stop it first",
                                database.name()
                            ),
                        )
                    })?;
                    let mut connection = database.connect()?;
                    super::migrate(&mut connection)?;
                    Ok((lock, connection))
                });
                // The lock file stays locked until `_lock` drops when this thread ends.
                let (_lock, mut connection) = match opened {
                    Ok(opened) => {
                        let _ = ready_tx.send(Ok(()));
                        opened
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };
                for job in queue {
                    job(&mut connection);
                }
            })?;
        ready_rx.recv().map_err(|_| io::Error::other("database thread ended during startup"))??;
        Ok(Self { jobs: Some(jobs), thread: Some(thread) })
    }

    /// Queues `job`; it runs after every job queued before it.
    pub fn submit(&self, job: impl FnOnce(&mut Connection) + Send + 'static) {
        if let Some(jobs) = &self.jobs {
            let _ = jobs.send(Box::new(job));
        }
    }

    /// Queues `job` and waits for its result (startup loads, character creation).
    pub fn call<T: Send + 'static>(
        &self,
        job: impl FnOnce(&mut Connection) -> io::Result<T> + Send + 'static,
    ) -> io::Result<T> {
        let (tx, rx) = mpsc::channel();
        self.submit(move |connection| {
            let _ = tx.send(job(connection));
        });
        rx.recv().map_err(|_| io::Error::other("database thread ended"))?
    }

    /// Waits until every queued job has run.
    pub fn drain(&self) -> io::Result<()> {
        self.call(|_| Ok(()))
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        drop(self.jobs.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
