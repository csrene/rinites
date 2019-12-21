use std::sync::{Arc, Mutex};
use std::fs::{OpenOptions, File, DirEntry};
use std::io::{Write, Seek, BufRead};
use std::io::SeekFrom;
use std::io::BufReader;
use std::sync::mpsc::{Receiver, Sender};
use crate::udp_server::{Request, ShardIteratorType};
use std::net::SocketAddr;
use crate::Response;
use std::thread::JoinHandle;
use std::{thread, fs};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct Record(pub Vec<u8>);

impl Record {
    pub fn from(base64_data: &[u8]) -> Record {
        let mut res = Vec::with_capacity(base64_data.len() + 1);
        base64_data.iter().for_each(|b| res.push(b.clone()));
        res.push(b'\n');
        Record(res)
    }
}




// We use an Arc to distribute the references with other threads, we use a mutex to make it so only one thread ever has acces to the file each time
//pub type ProtectedShardId = Arc<Mutex<ShardId>>;
pub type ShardId = u64;

type ShardOffset = u64;
// This is very strange, since it can be thought of as the shard iterator a client uses for a request
pub struct ShardReader {
    pub shard_id: ShardId,
    pub latest_shard: usize,
    pub offset: ShardOffset,
    pub chunk_size: usize,
    pub shard_dir: ShardDir
}


pub struct ShardWriter {
    pub latest_shard: Arc<AtomicUsize>,
    pub shard_dir: ShardDir,
    pub offset: ShardOffset,
    pub max_shard_size: u64
}

impl ShardWriter {

    pub fn append(&mut self, data: &[u8]) -> std::io::Result<()> {


        let mut file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .append(true)
            .open(self.shard_dir.path_to_shard(self.latest_shard.load(Ordering::Relaxed) as u64)).expect("could not open file to append");
        let record = Record::from(data);
        let written = file.write(&record.0)?;
        self.offset += written as u64;
        if self.offset > self.max_shard_size {
            let old_size = self.latest_shard.load(Ordering::Relaxed);
            self.latest_shard.store(self.offset as usize + old_size, Ordering::Relaxed);
            self.offset = 0;
            println!("releasing lock for new partition");
        }
        Ok(())
    }


}

impl ShardReader {
    pub fn read(&mut self) -> std::io::Result<Vec<Record>>  {

        let path = self.shard_dir.path_to_shard(self.shard_id);

        dbg!(&path);

        let mut res = Vec::new();

        let mut f = File::open(path)?;
        let mut reader = BufReader::new(f);
        reader.seek(SeekFrom::Start(self.offset));

        // FIXME this is unreadable
        loop {
            let mut b = String::new();
            let added_offset = reader.read_line(&mut b)?;
            if added_offset == 0 && self.shard_id < (self.latest_shard as u64) {
                f = File::open(self.shard_dir.path_to_shard(self.shard_id + self.offset)).unwrap();
                reader = BufReader::new(f);
                continue
            }
            self.offset += added_offset as u64;

            if b.ends_with('\n') {
                println!("removed trailing newline");
                b.pop();
            }

            res.push(Record(b.as_bytes().to_vec()));

            if res.len() >= self.chunk_size {
                break
            }
        }

        Ok(res)
    }
}

#[derive(Clone, Debug)]
pub struct ShardDir {
    pub mount_dir: PathBuf
}

#[derive(Debug)]
enum Shard {
    Latest,
    Old((String, u64))
}

impl ShardDir {

    fn path_to_shard(&self, shard_id: ShardId) -> PathBuf {
        self.mount_dir.join(format!("{:08}",shard_id))
    }

    fn get_latest_shard(&self) -> ShardId {
        let paths = fs::read_dir(&self.mount_dir).unwrap();

        let path = paths
            .map(|p|
                p.unwrap()
                    .file_name()
                    .into_string()
                    .unwrap())
            .max();

        path.expect("missing shards!!!!!! none found").parse().expect("file not in shard format1111")
    }

    fn get_oldest_shard(&self) -> ShardId {
        let paths = fs::read_dir(&self.mount_dir).unwrap();

        let path = paths.map(|p| p.unwrap().file_name().into_string().unwrap()).min();

        path.expect("missing shards!!!!!! none found").parse().expect("not in shard style")
    }

    fn find_belonging_shard(&self, shit: u64) -> (ShardId, ShardOffset) {

        let paths = fs::read_dir(&self.mount_dir).unwrap();
        dbg!(&paths);
        let mut candidate_shard_id: u64 = 0;
        let mut candidate_shard_id_offset: i64 = i64::max_value();

        for p in paths.map(|x|x.unwrap()) {
            dbg!(&p);
            let n: i64 = p.file_name().into_string().unwrap().parse().unwrap();
            let diff = (shit as i64) - n;
            dbg!(diff);
            if diff >= 0 && diff <= candidate_shard_id_offset {
                candidate_shard_id = n as u64;
                candidate_shard_id_offset = diff;
            }
            dbg!(candidate_shard_id);
            dbg!(candidate_shard_id_offset);

        }

        (candidate_shard_id, candidate_shard_id_offset as u64)
    }

    fn create_first_partition(&self) {
        let path = self.path_to_shard(0);
        dbg!(&path);
        File::create(&path).expect(&format!("could not create file {}", &path.to_string_lossy()));
    }

    fn assert_mount_path(&self) {
        if self.mount_dir.exists() && !self.mount_dir.is_dir() {
            panic!("mount path exists and is not a directory")
        }

        println!("creating mounting dir in {}", &self.mount_dir.to_string_lossy());
        fs::create_dir(&self.mount_dir);

        let paths = fs::read_dir(&self.mount_dir).unwrap();
        let valid_paths: Vec<DirEntry> = paths.map(|x| x.expect("invalid record")).collect();
        match valid_paths.len() {
            0 => {
                println!("about to create first partition");
                self.create_first_partition()
            },
            x => println!("all is ok, found partitions"),

        }
    }

    pub fn get_end_offset(&self, shard_id: &ShardId) -> u64 {

        let f = File::open(self.path_to_shard(*shard_id)).unwrap();
        let mut reader = BufReader::new(f);
        reader.seek(SeekFrom::End(0)).unwrap()
    }

}

pub fn start_shard_workers(
    shard_dir: ShardDir,
    task_rx: Arc<Mutex<Receiver<(Request, SocketAddr)>>>,
    response_tx: Sender<(Response, SocketAddr)>
) -> Vec<JoinHandle<()>> {

    shard_dir.assert_mount_path();
    let latest_shard = shard_dir.get_latest_shard();
    let latest_shard_offset = shard_dir.get_end_offset(&latest_shard);

    let latest_shard: Arc<AtomicUsize> = Arc::new(AtomicUsize::new(latest_shard as usize));
    let shard_writer = ShardWriter { latest_shard: latest_shard.clone(), shard_dir: shard_dir.clone(), offset: latest_shard_offset , max_shard_size: 1000000000 };
    let latest_shard_writer = Arc::new(Mutex::new(shard_writer));


    let mut threads = Vec::new();
    for n in 0..8 {
        let latest_shard_writer = latest_shard_writer.clone();
        let latest_shard = latest_shard.clone();

        let task_rx = task_rx.clone();
        let response_tx = response_tx.clone();
        let shard_dir = shard_dir.clone();
        let x = thread::spawn(move || {

            println!("started thread {}", n);
            loop {
                let (req, addr) = {
                    let rx = task_rx.lock().unwrap();
                    println!("got rx lock in thread {}", n);
                    rx.recv().unwrap()

                };

                println!("got req in thread {}", n);
                let response = handle_request(&latest_shard_writer, &latest_shard, &shard_dir, req).unwrap();
                dbg!(&response);
                println!("sending from thread {}", n);
                if let Err(e) = response_tx.send((response, addr)) {
                    println!("got err: {:?}", e);
                }

//                println!("waiting for udp lock on thead {}", n);
            }

        });
        threads.push(x);
    }
    threads
}

/// last_shard_id and last_shard_id_offset as atomic OR last_sequence_number as atomic.
/// i dont need lock on read, but i have to keep track of the offset of the last stable line. stable meaning it is not being written to
/// benchmark
/// use arc for shard dir
pub fn handle_request(latest_shard_writer: &Arc<Mutex<ShardWriter>>, latest_shard: &Arc<AtomicUsize>, shard_dir: &ShardDir ,request: Request) -> Result<Response, String> {
    match request {
        Request::GetRecords(shit) => {
            let shard_dir = shard_dir.clone();
            let (shard_id, offset): (ShardId, ShardOffset) = shard_dir.find_belonging_shard(shit);
            // FIXME do maths to figure out offset
            let mut reader: ShardReader = ShardReader { shard_id: shard_id, offset: offset, chunk_size: 10, shard_dir: shard_dir, latest_shard: latest_shard.load(Ordering::Relaxed) };
            let records: Vec<Record> = reader.read().unwrap();
            println!("read {} records", records.len());

            if records.len() == 0 {
                Ok(Response(format!("no more records remaining. offset was: {}", reader.offset)))
            } else {
                let text: Vec<&str> = records.iter().map(|r| std::str::from_utf8(&r.0).unwrap()).collect();
                Ok(Response(format!("next shard iterator: {}, records: {:?}",reader.offset + shard_id, text)))
            }

        },
        Request::GetShardIterator(ShardIteratorType::Latest) => {
            let shit = shard_dir.get_latest_shard();
            let result = Response(format!("shard iterator: {}", shit));
            Ok(result)
        },
        Request::GetShardIterator(ShardIteratorType::Oldest) => {
            let shit = shard_dir.get_oldest_shard();
            let result = Response(format!("shard iterator: {}", shit));
            Ok(result)
        },
        Request::PutRecords(record) => {
            let mut writer = latest_shard_writer.lock().unwrap();
            writer.append(&record).map_err(|x| x.to_string()).map(|nothing| Response("data was written!!!".to_string()))
        }
    }
}