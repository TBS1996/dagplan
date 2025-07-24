mod slot;

use crossterm::cursor::{MoveLeft, MoveTo, MoveToColumn};
use crossterm::event::{self, read, Event, KeyCode, MediaKeyCode};
use crossterm::execute;
use crossterm::style::{Attribute, Print, SetAttribute};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, Clear, ClearType};
use serde::de::DeserializeOwned;
use slot::{calculate_slots, dur, t, SlotDto, SlotResult};
use std::collections::{HashMap, VecDeque};
use std::fmt::Display;
use std::hash::Hash;
use std::ops::ControlFlow;
use std::path::PathBuf;
use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::{fs, sync::Arc};
use uuid::Uuid;

use vedvaring::{DefaultWithId, FsTrait, Saved};

use chrono::{Duration, Local, NaiveDate, NaiveDateTime, NaiveTime, Timelike, Utc};

use serde::{Deserialize, Serialize};

type ActId = Uuid;
type SlotId = Uuid;

use crossterm::{
    cursor,
    style::{self, Stylize},
    terminal, ExecutableCommand, QueueableCommand,
};
use std::io::{self, Stdout, Write};

fn current_time() -> NaiveTime {
    Local::now().time()
}

fn current_day() -> NaiveDate {
    Local::now().date_naive()
}

#[derive(Default)]
pub struct SingletonCache<K: PartialEq + Clone, V>(RwLock<Option<(K, Arc<V>)>>);

impl<K: PartialEq + Clone, V> SingletonCache<K, V> {
    pub fn get(&self, key: &K, f: Box<dyn Fn(&K) -> V>) -> Arc<V> {
        if let Some((inner_key, val)) = &*self.0.read().unwrap() {
            if inner_key == key {
                return val.clone();
            }
        }

        let new_val = Arc::new(f(key));
        *self.0.write().unwrap() = Some((key.clone(), new_val.clone()));
        new_val
    }
}

fn main() {
    let date = NaiveDate::from_ymd_opt(2025, 3, 28).unwrap();
    println!("Date: {}", date);

    let mut app = App::start();

    enable_raw_mode().unwrap();
    app.run();
    disable_raw_mode().unwrap();
    libnotify::uninit();
}

#[derive(Copy, Clone, Default)]
struct Cursor {
    index: usize,
    field: Field,
}

impl Cursor {
    fn up(&mut self) {
        if self.index > 0 {
            self.index -= 1;
        }
    }
    fn down(&mut self, slot_qty: usize) {
        if self.index < slot_qty - 1 {
            self.index += 1;
        }
    }

    fn left(&mut self) {
        self.field = match self.field {
            Field::Name => Field::Name,
            Field::Start => Field::Name,
            Field::Requested => Field::Start,
            Field::Length => Field::Requested,
        };
    }
    fn right(&mut self) {
        self.field = match self.field {
            Field::Name => Field::Start,
            Field::Start => Field::Length,
            Field::Requested => Field::Length,
            Field::Length => Field::Length,
        };
    }
}

fn print_styled(stdout: &mut Stdout, text: &str, attrs: Vec<Attribute>) -> io::Result<()> {
    stdout.execute(SetAttribute(Attribute::Reset))?;

    for attr in attrs {
        stdout.execute(SetAttribute(attr))?;
    }
    stdout.execute(Print(text))?;

    // Reset style after printing
    stdout.execute(SetAttribute(Attribute::Reset))?;
    Ok(())
}

#[derive(Copy, Clone, Default, Eq, PartialEq)]
enum Field {
    #[default]
    Name,
    Length,
    Start,
    Requested,
}

struct App {
    stdout: Stdout,
    cursor: Cursor,
    selected_day: Saved<Day>,
    days: HashMap<NaiveDate, Saved<Day>>,
}

enum Action {
    Down,
    Up,
    Left,
    Right,
    Tomorrow,
    Yesterday,
    Insert,
    Delete,
    Quit,
    Edit,
    Upswap,
    Downswap,
    Begin,
}

impl Action {
    fn from_event(event: Event) -> Option<Self> {
        let Event::Key(key) = event else {
            return None;
        };

        use crossterm::event::KeyCode as KC;

        match key.code {
            KC::Backspace => None,
            KC::Enter => Action::Edit.into(),
            KC::Left => Some(Action::Left),
            KC::Right => Some(Action::Right),
            KC::Up => Some(Action::Up),
            KC::Down => Some(Action::Down),
            KC::Home => None,
            KC::End => None,
            KC::PageUp => None,
            KC::PageDown => None,
            KC::Tab => None,
            KC::BackTab => None,
            KC::Delete => Some(Action::Delete),
            KC::Insert => Some(Action::Insert),
            KC::F(_) => None,
            KC::Char('j') => Some(Action::Down),
            KC::Char('k') => Some(Action::Up),
            KC::Char('h') => Some(Action::Left),
            KC::Char('l') => Some(Action::Right),
            KC::Char('i') => Some(Action::Insert),
            KC::Char('q') => Some(Action::Quit),
            KC::Char('r') => Some(Action::Upswap),
            KC::Char('f') => Some(Action::Downswap),
            KC::Char('b') => Some(Action::Begin),
            KC::Char('m') => Some(Action::Tomorrow),
            KC::Char('n') => Some(Action::Yesterday),
            KC::Char(_) => None,
            KC::Null => None,
            KC::Esc => Some(Action::Quit),
            KC::CapsLock => None,
            KC::ScrollLock => None,
            KC::NumLock => None,
            KC::PrintScreen => None,
            KC::Pause => None,
            KC::Menu => None,
            KC::KeypadBegin => None,
            KC::Media(_) => None,
            KC::Modifier(_) => None,
        }
    }
}

impl App {
    fn clear_screen(&mut self) {
        execute!(&mut self.stdout, Clear(ClearType::All), MoveTo(0, 0)).unwrap();
    }

    fn flush(&mut self) {
        self.stdout.flush().unwrap();
    }

    fn left_cursor(&mut self) {
        execute!(self.stdout, MoveToColumn(0)).unwrap();
    }

    fn get_user_input(&mut self, prompt: impl AsRef<str>) -> io::Result<String> {
        self.clear_screen();
        print!("{}: ", prompt.as_ref());
        self.flush();
        let mut input = String::new();

        loop {
            if let Event::Key(event) = read()? {
                match event.code {
                    KeyCode::Char(c) => {
                        input.push(c);
                        print!("{}", c);
                        self.stdout.flush()?;
                    }
                    KeyCode::Backspace => {
                        if input.pop().is_some() {
                            execute!(self.stdout, MoveLeft(1))?;
                            print!(" ");
                            execute!(self.stdout, MoveLeft(1))?;
                            self.stdout.flush()?;
                        }
                    }
                    KeyCode::Enter => {
                        println!();
                        break;
                    }
                    _ => {}
                }
            }
        }
        Ok(input)
    }

    fn get_int(&mut self, prompt: impl AsRef<str>) -> Option<u32> {
        loop {
            let s = self.get_user_input(&prompt).unwrap();
            if s.is_empty() {
                return None;
            };

            if let Ok(num) = s.parse::<u32>() {
                return Some(num);
            }
        }
    }

    fn get_naivetime(&mut self, prompt: impl AsRef<str>) -> Option<NaiveTime> {
        loop {
            let s = self.get_user_input(&prompt).unwrap();
            if s.is_empty() {
                return None;
            };

            if let Ok(time) = NaiveTime::parse_from_str(&s, "%H:%M") {
                return Some(time);
            }
        }
    }

    pub fn start() -> Self {
        let today = current_day();
        let day = Saved::load_or_create(today);
        let mut days: HashMap<NaiveDate, Saved<Day>> = Default::default();
        days.insert(today, day.clone());

        Self {
            stdout: io::stdout(),
            selected_day: day,
            days,
            cursor: Cursor::default(),
        }
    }

    pub fn load_or_create(&mut self, dayte: NaiveDate) {
        if let Some(day) = self.days.get(&dayte).cloned() {
            self.selected_day = day;
        } else {
            let day = Saved::load_or_create(dayte);
            self.days.insert(dayte, day.clone());
            self.selected_day = day;
        }
    }

    fn current_index(&self) -> Option<usize> {
        let mut slots = self.selected_day.read().slots_config.clone();
        if slots.is_empty() {
            return None;
        };

        Some(self.cursor.index.clamp(0, slots.len() - 1))
    }

    fn handle_action(&mut self, action: Action) -> ControlFlow<()> {
        match action {
            Action::Down => self
                .cursor
                .down(self.selected_day.read().slots_config.len()),
            Action::Up => self.cursor.up(),
            Action::Left => self.cursor.left(),
            Action::Right => self.cursor.right(),
            Action::Tomorrow => {
                let next_day = self.selected_day.read().day.succ_opt().unwrap();
                self.load_or_create(next_day);
            }
            Action::Yesterday => {
                let prev_day = self.selected_day.read().day.pred_opt().unwrap();
                self.load_or_create(prev_day);
            }
            Action::Insert => {
                self.selected_day.write().insert(self.cursor);
            }
            Action::Delete => {
                if let Some(idx) = self.current_index() {
                    self.selected_day.write().slots_config.remove(idx);
                }
            }
            Action::Quit => return ControlFlow::Break(()),
            Action::Edit => {
                let mut slots = self.selected_day.read().slots_config.clone();
                if slots.is_empty() {
                    return ControlFlow::Continue(());
                };

                let idx = self.cursor.index.clamp(0, slots.len() - 1);
                let mut selected_slot = slots.get(idx).unwrap().clone();
                match self.cursor.field {
                    Field::Name => {
                        let name = self.get_user_input("activity name").unwrap();
                        selected_slot.name = name;
                    }
                    Field::Length => {
                        selected_slot.config.fixed_length = !selected_slot.config.fixed_length;
                    }
                    Field::Start => {
                        if selected_slot.config.start.is_some() {
                            selected_slot.config.start = None;
                        } else {
                            if let Some(time) = self.get_naivetime("set starttime") {
                                selected_slot.config.start = Some(time);
                            } else {
                                return ControlFlow::Continue(());
                            }
                        }
                    }
                    Field::Requested => match self.get_int("length in minutes") {
                        Some(num) => selected_slot.config.length = Duration::minutes(num as i64),
                        None => return ControlFlow::Continue(()),
                    },
                }

                slots[idx] = selected_slot;
                self.selected_day.write().slots_config = slots;
            }
            Action::Upswap => {
                let mut slots = self.selected_day.read().slots_config.clone();
                if slots.is_empty() {
                    return ControlFlow::Continue(());
                };

                let idx = self.cursor.index.clamp(0, slots.len() - 1);

                if idx > 0
                    && idx < slots.len()
                    && !(idx == 1 && slots.get(idx - 1).unwrap().config.start.is_some())
                {
                    slots.swap(idx, idx - 1);
                    self.selected_day.write().slots_config = slots;
                    self.cursor.up();
                }
            }
            Action::Downswap => {
                let mut slots = self.selected_day.read().slots_config.clone();
                if slots.is_empty() {
                    return ControlFlow::Continue(());
                };

                let idx = self.cursor.index.clamp(0, slots.len() - 1);

                if idx + 1 < slots.len() {
                    slots.swap(idx, idx + 1);
                }

                self.selected_day.write().slots_config = slots;
                self.cursor
                    .down(self.selected_day.read().slots_config.len());
            }
            Action::Begin => {
                let mut slots = self.selected_day.read().slots_config.clone();
                if slots.is_empty() {
                    return ControlFlow::Continue(());
                };

                let idx = self.cursor.index.clamp(0, slots.len() - 1);
                let mut selected_slot = slots.get(idx).unwrap().clone();

                selected_slot.config.start = Some(current_time());
                slots[idx] = selected_slot;

                self.selected_day.write().slots_config = slots;
            }
        }

        ControlFlow::Continue(())
    }

    fn draw(&mut self) {
        self.clear_screen();
        println!("{}", self.selected_day.read().day);
        self.left_cursor();
        let slots = self.selected_day.read().slots();
        if slots.is_empty() {
            print!("empty...");
            return;
        }
        let index = self.cursor.index.clamp(0, slots.len() - 1);

        let current_time = current_time();

        let max_name_len: usize = slots
            .iter()
            .map(|slot| slot.configured.name.chars().count())
            .max()
            .unwrap_or_default();
        let name_width = max_name_len.max(15);

        for (i, slot) in slots.iter().enumerate() {
            for field in [Field::Name, Field::Start, Field::Requested, Field::Length] {
                let s = match field {
                    Field::Name => format!(
                        "{:width$}",
                        slot.configured.name.clone(),
                        width = name_width
                    ),
                    Field::Length => format_dur(slot.length),
                    Field::Start => format_naive(slot.start),
                    Field::Requested => format_dur(slot.configured.config.length),
                };

                let mut attrs = vec![];
                if self.cursor.field == field && i == index {
                    attrs.push(Attribute::Reverse);
                }

                if (field == Field::Start && slot.configured.config.start.is_some())
                    || (field == Field::Requested && slot.configured.config.fixed_length)
                {
                    attrs.push(Attribute::Bold);
                }

                print_styled(&mut self.stdout, &s, attrs).unwrap();
                print!("   ");
            }

            if self.selected_day.read().day == current_day()
                && slot.start < current_time
                && (slot.start + slot.length) > current_time
            {
                let clock = clock_emoji(current_time);
                print!("{clock}");
            }

            println!();
            self.left_cursor();
        }

        self.flush();
    }

    fn current_slot(&self) -> Option<SlotResult> {
        let slots = self.days.get(&current_day())?.read().slots();
        let now = current_time();

        for slot in slots.iter() {
            if slot.start < now && (slot.start + slot.length) > now {
                return Some(slot.clone());
            }
        }

        None
    }

    pub fn run(&mut self) {
        libnotify::init("dayplanner").unwrap();
        self.stdout
            .execute(terminal::Clear(terminal::ClearType::All))
            .unwrap();

        self.draw();
        let mut current_slot = self.current_slot();
        if let Some(slot) = current_slot.clone() {
            write_slot(&slot);
        }
        loop {
            self.draw();
            self.draw();
            let event = match timed_input(5) {
                Some(event) => {
                    let new_slot = self.current_slot();
                    if current_slot != new_slot {
                        if let Some(slot) = &new_slot {
                            on_new_slot(&slot);
                        }
                        current_slot = new_slot;
                    }

                    event

                },
                None => {
                    let new_slot = self.current_slot();
                    if current_slot != new_slot {
                        if let Some(slot) = &new_slot {
                            on_new_slot(&slot);
                        }
                        current_slot = new_slot;
                    }

                    continue;
                }
            };
            let Some(action) = Action::from_event(event) else {
                continue;
            };

            if self.handle_action(action).is_break() {
                return;
            }
        }
    }
}

fn write_slot(slot: &SlotResult) {
    use std::io::Write;
    let mut f = std::fs::File::create(dirs::home_dir().unwrap().join(".current_task")).unwrap();
    f.write_all(slot.configured.name.as_bytes()).unwrap();

}

fn on_new_slot(slot: &SlotResult) {
    write_slot(slot);

    let s = format!("new task: {}", &slot.configured.name);
    libnotify::Notification::new(s.as_str(), None, None)
        .show()
        .unwrap();
}



pub fn timed_input(timeout_secs: u64) -> Option<Event> {
    if event::poll(std::time::Duration::from_secs(timeout_secs)).ok()? {
        event::read().ok()
    } else {
        None
    }
}

fn clock_emoji(time: NaiveTime) -> char {
    let (hour, minute) = hour_and_minute(time);
    let rounded_hour = match minute {
        0..=14 => hour,
        15..=44 => hour, // or return half-hour later if you want 🕦
        _ => hour + 1,
    } % 12;

    let base = 0x1F550;
    let codepoint = base
        + if rounded_hour == 0 {
            11
        } else {
            rounded_hour - 1
        };

    std::char::from_u32(codepoint).unwrap_or('🕛') // fallback just in case
}

fn hour_and_minute(time: NaiveTime) -> (u32, u32) {
    let secs = time.num_seconds_from_midnight();
    let hours = secs / 3600;
    let minutes = (secs % 3600) / 60;
    (hours, minutes)
}

fn format_dur(dur: Duration) -> String {
    let mins = dur.num_seconds() / 60;
    format!("{:>5}m", mins)
}

fn format_naive(time: NaiveTime) -> String {
    let (hours, minutes) = hour_and_minute(time);
    format!("{:02}:{:02}", hours, minutes)
}

#[derive(Serialize, Deserialize)]
struct Day {
    day: NaiveDate,
    slots_config: Vec<SlotDto>,
    #[serde(skip)]
    slot_result: SingletonCache<Vec<SlotDto>, Vec<SlotResult>>,
}

impl DefaultWithId for Day {
    fn default_with_id(id: Self::Key) -> Self {
        Self {
            day: id,
            slots_config: vec![],
            slot_result: Default::default(),
        }
    }
}

impl Day {
    fn insert(&mut self, cursor: Cursor) {
        let index = cursor.index.clamp(0, self.slots_config.len());
        let new_slot = SlotDto::default();
        self.slots_config.insert(index, new_slot);
    }

    fn slots(&self) -> Arc<Vec<SlotResult>> {
        let f: Box<dyn Fn(&Vec<SlotDto>) -> Vec<SlotResult>> =
            Box::new(|slots: &Vec<SlotDto>| calculate_slots(t(7, 0), dur(16 * 60), slots.clone()));

        self.slot_result.get(&self.slots_config, f)
    }
}

impl FsTrait for Day {
    type Key = NaiveDate;

    fn item_id(&self) -> Self::Key {
        self.day
    }
}

/// An activity, not tied to a specific instance, can be shared between days and slots
#[derive(Serialize, Deserialize, Debug)]
struct Act {
    name: String,
    id: ActId,
}

impl FsTrait for Act {
    type Key = ActId;

    fn item_id(&self) -> Self::Key {
        self.id
    }
}

#[derive(Serialize, Deserialize, Default, Debug)]
struct DayDto {
    day: NaiveDate,
    slots: Vec<SlotId>,
}

impl FsTrait for DayDto {
    type Key = NaiveDate;

    fn item_id(&self) -> Self::Key {
        self.day
    }
}
