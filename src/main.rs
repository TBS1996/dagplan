use serde::de::DeserializeOwned;
use uuid::Uuid;
use std::fmt::Display;
use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::{fs, sync::Arc};
use std::collections::HashMap;
use std::path::PathBuf;

use chrono::NaiveDate;

use serde::{Serialize, Deserialize};

type ActId = Uuid;
type SlotId = Uuid;
type DayNumber = u64;


fn root_path() -> PathBuf {
    let root = PathBuf::from("/home/.local/share/dayplanner");
    fs::create_dir_all(&root).unwrap();
    root
}

fn main() {
    let date = NaiveDate::from_ymd_opt(2025, 3, 28).unwrap();
    println!("Date: {}", date);
}

struct App {
    days: HashMap<NaiveDate, Day>,
    root: PathBuf,
}

struct Day {
    slots: Vec<ActSlot>,
}

impl Day {
    fn load(day: NaiveDate) -> Self {
        let dto = DayDto::load(day).unwrap_or_default();
        todo!()
    }
}

struct ActSlot {
    id: SlotId,
    act: Act,
    slot_config: TimeSlotConfig,
    start: u64,
    length: u64,
}

#[derive(Serialize, Deserialize, Debug)]
struct SlotDto {
    id: SlotId,
    act: ActId,
    config: TimeSlotConfig,
}

impl SlotDto {
    fn slot_path() -> PathBuf {
        let path = root_path().join("acts");
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn load(id: ActId) -> Option<Self> {
        let path = Self::slot_path().join(id.to_string());
        if !path.exists(){
            return None;
        } 

        let s = fs::read_to_string(&path).unwrap();
        Some(serde_json::from_str(&s).unwrap())
    }
}


/// The configuration for when a slot should be. Doesn't mean it will be on that time that depends on its constraints
#[derive(Serialize, Deserialize, Debug)]
struct TimeSlotConfig {
    start: u64,
    length: u64,
    fixed_length: bool,
    fixed_time: bool,
}

/// An activity, not tied to a specific instance, can be shared between days and slots
#[derive(Serialize, Deserialize, Debug)]
struct Act {
    name: String,
    id: ActId,
}

impl Act {
    fn act_path() -> PathBuf {
        let path = root_path().join("acts");
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn load(id: ActId) -> Option<Self> {
        let path = Self::act_path().join(id.to_string());
        if !path.exists(){
            return None;
        } 

        let s = fs::read_to_string(&path).unwrap();
        Some(serde_json::from_str(&s).unwrap())
    }
}


#[derive(Serialize, Deserialize, Default, Debug)]
struct DayDto {
    slots: Vec<SlotId>,
}

impl DayDto {
    fn day_paths() -> PathBuf {
        let path = root_path().join("days");
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn load(day: NaiveDate) -> Option<Self> {
        let path = Self::day_paths().join(format!("{day}"));
        if !path.exists(){
            return None;
        } 

        let s = fs::read_to_string(&path).unwrap();
        Some(serde_json::from_str(&s).unwrap())
    }
}

use std::ops::Deref;


#[derive(Clone, Debug)]
pub struct Saved<T: FsTrait>(Arc<RwLock<T>>);


impl<T: FsTrait> Saved<T> {
    pub fn new(item: T) -> Self {
        item.save();
        Self(Arc::new(RwLock::new(item)))
    }

    pub fn load(id: T::Key) -> Option<Self> {
        let item = T::load(id)?;
        Some(Self(Arc::new(RwLock::new(item))))
    }

    pub fn read(&self) -> RwLockReadGuard<T> {
        self.0.read().unwrap()
    }

    pub fn write(&self) -> MyWriteGuard<T>{
        MyWriteGuard(self.0.write().unwrap())
    }
}


/// Wrapper for writeguard which saves the item to disk when the writeguard goes out of scope.
pub struct MyWriteGuard<'a, T: FsTrait>(RwLockWriteGuard<'a, T>);

impl<'a, T: FsTrait> Drop for MyWriteGuard<'a, T> {
    fn drop(&mut self) {
        self.save();
    }
}

impl<'a, T: FsTrait> Deref for MyWriteGuard<'a, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}


trait FsTrait where Self: DeserializeOwned + Serialize + Sized {
    type Key: Display;

    fn item_id(&self) -> Self::Key;

    fn crate_name() -> String {
        std::env::current_exe().unwrap().file_stem().unwrap().to_str().unwrap().to_string().to_lowercase()
    }

    fn root() -> PathBuf {
        dirs::data_local_dir().unwrap().join(Self::crate_name())
    }


    fn items_path() -> PathBuf {
        let name = std::any::type_name::<Self>();
        let path = Self::root().join(name);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn item_path(&self) -> PathBuf {
        Self::items_path().join(self.item_id().to_string())
    }


    fn load(id: Self::Key) -> Option<Self> {
        let path = Self::items_path().join(id.to_string());
        if !path.exists() {
            return None;
        }

        let s: String = fs::read_to_string(&path).unwrap();
        let t: Self = serde_json::from_str(&s).unwrap();
        Some(t)
    }

    fn save(&self) {
        use std::io::Write;

        let path = self.item_path();
        let s: String = serde_json::to_string(self).unwrap();
        let mut f = fs::File::create(&path).unwrap();
        f.write_all(&s.as_bytes()).unwrap();
    }
}