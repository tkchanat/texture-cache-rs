use std::{collections::HashMap, ffi::OsStr};

pub struct FdEntry {
    file: std::fs::File,
    tex_id: u32,
    index: usize,
}

impl std::ops::Deref for FdEntry {
    type Target = std::fs::File;

    fn deref(&self) -> &Self::Target {
        &self.file
    }
}

impl std::ops::DerefMut for FdEntry {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.file
    }
}

pub struct FdCache {
    files: Box<[Option<FdEntry>]>,
    free_list: Vec<usize>,
    in_used: HashMap<u32, usize>,
}

impl FdCache {
    pub fn new(max_opened_files: usize) -> Self {
        Self {
            files: (0..max_opened_files).map(|_| None).collect::<Box<_>>(),
            free_list: (0..max_opened_files).collect(),
            in_used: HashMap::default(),
        }
    }

    pub fn look_up(&mut self, tex_id: u32, path: &OsStr) -> FdEntry {
        // return existing opened file descriptor with matching tex_id
        if let Some(index) = self.in_used.get(&tex_id) {
            return self.files[*index].take().unwrap();
        }
        // or else, get a slot from the free_list and open the file descriptor
        let index = self
            .free_list
            .pop()
            .expect("Running out of free file descriptors");
        let file =
            std::fs::File::open(path).expect(&format!("Unable to open file at path '{path:?}'"));
        self.in_used.insert(tex_id, index);
        if let Some(entry) = self.files[index].replace(FdEntry {
            file,
            tex_id,
            index,
        }) {
            self.in_used.remove(&entry.tex_id);
        }
        self.look_up(tex_id, path)
    }

    pub fn recycle(&mut self, entry: FdEntry) {
        let index = entry.index;
        self.free_list.push(index);
        self.files[index] = Some(entry);
    }
}
