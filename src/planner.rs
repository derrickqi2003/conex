use core::panic;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{self};
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::cmp;


pub struct ConexPlanner {
    pub layer_to_files: Vec<(String, Vec<ConexFile>)>,
    pub split_threshold: usize,
}

#[derive(Clone, Debug)]
pub struct ConexFile {
    pub path: PathBuf,
    pub relative_path: PathBuf,
    pub size: usize,
    pub inode: u64,
    pub hard_link_to: Option<PathBuf>,
    pub ctime_nsec: i64,
    pub start_offset: Option<usize>,
    pub chunk_size: Option<usize>,
}

impl ConexPlanner {
    pub fn default() -> Self {
        Self {
            layer_to_files: Vec::new(),
            split_threshold: 512 * 1024 * 1024,
        }
    }

    pub fn ingest_dir(&mut self, dir_path: &str) {
        let base_path = PathBuf::from(dir_path.clone());

        if base_path.metadata().is_err()
            && base_path.metadata().err().unwrap().kind() == std::io::ErrorKind::PermissionDenied
        {
            panic!(
                "Path is not accessible.
            Run `sudo setfacl -m u:ubuntu:rx /var /var/lib /var/lib/docker`
            and `sudo setfacl -R -m u:ubuntu:rx /var /var/lib /var/lib/docker/overlay2`
            "
            );
        }

        if !base_path.is_dir() {
            // TODO: change to log fatal.
            panic!("Path is not a directory");
        }

        let mut queue = VecDeque::new();
        queue.push_back(PathBuf::new());

        let mut file_metadata_vec = Vec::new();

        while let Some(current_path) = queue.pop_front() {
            let absolute_path = base_path.join(&current_path);
            for entry in fs::read_dir(&absolute_path).unwrap() {
                let entry = entry.unwrap();
                // let metadata = entry.metadata().unwrap();
                let metadata = std::fs::symlink_metadata(entry.path()).unwrap();
                let relative_path = entry.path().strip_prefix(&base_path).unwrap().to_path_buf();

                if entry.path().is_dir() && !metadata.is_symlink() {
                    queue.push_back(relative_path.to_owned());
                }

                file_metadata_vec.push(ConexFile {
                    path: entry.path(),
                    relative_path,
                    size: metadata.len() as usize,
                    inode: metadata.ino(),
                    hard_link_to: None,
                    ctime_nsec: metadata.ctime_nsec(),
                    start_offset: None,
                    chunk_size: None
                });
            }
        }

        self.layer_to_files
            .push((dir_path.to_owned(), file_metadata_vec));
    }

    pub fn generate_plan(mut self) -> Vec<(String, Vec<ConexFile>)> {
        // Pass 1: find all hard links, point them to the same inode.
        self.layer_to_files = self
            .layer_to_files
            .into_iter()
            .map(|(key, mut files)| {
                let mut inode_map: HashMap<u64, PathBuf> = HashMap::new();

                for file in files.iter_mut() {
                    if let Some(hard_link_to) = inode_map.get(&file.inode) {
                        file.hard_link_to = Some(hard_link_to.to_owned());
                    } else {
                        inode_map.insert(file.inode, file.relative_path.to_owned());
                    }
                }

                (key, files)
            })
            .collect::<Vec<(String, Vec<ConexFile>)>>();

        // Pass 2: Split and collapse layers so the size is about 512MB.
        let mut new_layer_to_files = Vec::new();
        let mut current_layer_size: usize = 0;
        let mut new_layer = Vec::new();
        for (layer, files) in self.layer_to_files.iter() {
            for file in files.iter() {
                let mut remainder_size = file.size;
                while remainder_size != 0 {
                    let mut frag = file.clone();
                    if frag.hard_link_to.is_none() || remainder_size + current_layer_size < self.split_threshold {
                        if remainder_size != file.size {
                            //Case where remainder is a leftover fragment
                            frag.chunk_size = Some(remainder_size);
                            frag.start_offset = Some(file.size - remainder_size);
                        }
                        new_layer.push(frag.to_owned());
                        current_layer_size += remainder_size;
                        break;
                    } else {
                        //Split file + layer
                        frag.start_offset = Some(file.size - remainder_size);
                        frag.chunk_size = Some(self.split_threshold - current_layer_size);
                        new_layer.push(frag.to_owned());
                        new_layer_to_files.push((layer.clone(), new_layer.clone()));
                        new_layer = Vec::new();
                        current_layer_size = 0;
                        remainder_size -= frag.chunk_size.unwrap();
                    } 
                }
            }
        }
        if !new_layer.is_empty() {
            new_layer_to_files.push((String::from("last"), (new_layer).clone()));
            //println!("last/first layer with size{}", new_layer.pop().unwrap().size.to_string());
            //layer_counter +=1;
        }
        //println!("{} layers created from {} layers given, plan len {}",layer_counter,num_layers, new_layer_to_files.len());
        new_layer_to_files.clone()
    }
}

// unit test module
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_split_layers() {
        let mut planner = ConexPlanner::default();

        planner.split_threshold = 100;


        // insert fake ConexFile to planner
        let mut files = Vec::new();
        files.push(ConexFile {
            path: PathBuf::from("/var/lib/docker/overlay2/123"),
            relative_path: PathBuf::from("123"),
            size: 100,
            inode: 1,
            hard_link_to: Some(PathBuf::new()),
            ctime_nsec: 0,
            start_offset: None,
            chunk_size: None
        });
        files.push(ConexFile {
            path: PathBuf::from("/var/lib/docker/overlay2/456"),
            relative_path: PathBuf::from("456"),
            size: 100,
            inode: 2,
            hard_link_to: Some(PathBuf::new()),
            ctime_nsec: 0,
            start_offset: None,
            chunk_size: None
        });
        files.push(ConexFile {
            path: PathBuf::from("/var/lib/docker/overlay2/789"),
            relative_path: PathBuf::from("789"),
            size: 100,
            inode: 3,
            hard_link_to: Some(PathBuf::new()),
            ctime_nsec: 0,
            start_offset: None,
            chunk_size: None
        });
        

        planner.layer_to_files.push(("/var/lib/docker/overlay2".to_owned(), files));


        let mut plan = planner.generate_plan();
        assert_eq!(plan.len(), 3, "plan is: {:?}", plan);
        let (_, t_files) = plan.pop().unwrap();
        let mut c_files = t_files.clone();
        assert_eq!(c_files.pop().unwrap().size, 100);
        let (_, t_files) = plan.pop().unwrap();
        let mut c_files = t_files.clone();
        assert_eq!(c_files.pop().unwrap().size, 100);
        let (holder, t_files) = plan.pop().unwrap();
        let mut c_files = t_files.clone();
        println!("layer name {}", holder);
        assert_eq!(c_files.pop().unwrap().size, 100);
    }
    #[test]
    fn test_merge_layers() {
        let mut planner = ConexPlanner::default();

        planner.split_threshold = 100;


        // insert fake ConexFile to planner
        let mut files = Vec::new();
        files.push(ConexFile {
            path: PathBuf::from("/var/lib/docker/overlay2/123"),
            relative_path: PathBuf::from("123"),
            size: 50,
            inode: 1,
            hard_link_to: Some(PathBuf::new()),
            ctime_nsec: 0,
            start_offset: None,
            chunk_size: None
        });
        planner.layer_to_files.push(("/var/lib/docker/overlay2".to_owned(), files.clone()));
        planner.layer_to_files.push(("/var/lib/docker/overlay2".to_owned(), files.clone()));


        let mut plan = planner.generate_plan();
        assert_eq!(plan.len(), 1, "plan is: {:?}", plan);
        let (_, t_files) = plan.pop().unwrap();
        let mut c_files = t_files.clone();
        assert_eq!(c_files.pop().unwrap().size, 50);
        assert_eq!(c_files.pop().unwrap().size, 50);
    }
    #[test]
    fn test_fragment_layers() {
        let mut planner = ConexPlanner::default();

        planner.split_threshold = 50;


        // insert fake ConexFile to planner
        let mut files = Vec::new();
        files.push(ConexFile {
            path: PathBuf::from("/var/lib/docker/overlay2/123"),
            relative_path: PathBuf::from("123"),
            size: 100,
            inode: 1,
            hard_link_to: Some(PathBuf::new()),
            ctime_nsec: 0,
            start_offset: None,
            chunk_size: None
        });
        
        planner.layer_to_files.push(("/var/lib/docker/overlay2".to_owned(), files));


        let plan = planner.generate_plan();
        assert_eq!(plan.len(), 2, "plan is: {:?}", plan);
    }
    #[test]
    fn test_merge_then_fragment_layers() {
        let mut planner = ConexPlanner::default();

        planner.split_threshold = 75;


        // insert fake ConexFile to planner
        let mut files = Vec::new();
        files.push(ConexFile {
            path: PathBuf::from("/var/lib/docker/overlay2/123"),
            relative_path: PathBuf::from("123"),
            size: 50,
            inode: 1,
            hard_link_to: Some(PathBuf::new()),
            ctime_nsec: 0,
            start_offset: None,
            chunk_size: None
        });
        
        planner.layer_to_files.push(("/var/lib/docker/overlay2".to_owned(), files.clone()));
        planner.layer_to_files.push(("/var/lib/docker/overlay2".to_owned(), files.clone()));


        let mut plan = planner.generate_plan();
        assert_eq!(plan.len(), 2, "plan is: {:?}", plan);
        let (_, t_files) = plan.pop().unwrap();
        let mut c_files = t_files.clone();
        assert_eq!(c_files.pop().unwrap().chunk_size.unwrap(), 25);
        let (_, t_files) = plan.pop().unwrap();
        let mut c_files = t_files.clone();
        assert_eq!(c_files.pop().unwrap().chunk_size.unwrap(), 25);
        assert_eq!(c_files.pop().unwrap().size, 50);
        
    }
}
