mod slot;

use crossterm::cursor::{MoveLeft, MoveTo, MoveToColumn};
use crossterm::event::{self, read, Event, KeyCode};
use crossterm::execute;
use crossterm::style::{Attribute, Print, SetAttribute};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, Clear, ClearType};
use notify_rust::Notification;
use slot::{calculate_slots, dur, t, SlotDto, SlotResult};
use std::collections::HashMap;
use std::ops::{ControlFlow, Deref};
use std::sync::Arc;
use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

type TimeSinceMidnight = Duration;

use vedvaring::{DefaultWithId, FsTrait, Saved};

use chrono::{Duration, Local, NaiveDate, NaiveTime};

use serde::{Deserialize, Serialize};

type ActId = Uuid;
type SlotId = Uuid;

use crossterm::{terminal, ExecutableCommand};
use std::io::{self, Stdout, Write};

const DAY_OFFSET_SEC: i64 = 3 * 60 * 60;

fn is_past_midnight() -> bool {
    let from_mid = Local::now()
        .time()
        .signed_duration_since(NaiveTime::from_hms_opt(0, 0, 0).unwrap());

    from_mid.num_seconds() < DAY_OFFSET_SEC
}

fn naive_to_timesincemidnight(naive: NaiveTime) -> TimeSinceMidnight {
    let from_mid = naive.signed_duration_since(NaiveTime::from_hms_opt(0, 0, 0).unwrap());

    let mut secs_since_midinght = from_mid.num_seconds();

    if secs_since_midinght < DAY_OFFSET_SEC {
        secs_since_midinght += DAY_OFFSET_SEC;
    }

    TimeSinceMidnight::seconds(secs_since_midinght)
}

fn current_time() -> TimeSinceMidnight {
    naive_to_timesincemidnight(Local::now().time())
}

fn current_day() -> NaiveDate {
    let mut day = Local::now().date_naive();

    if is_past_midnight() {
        day = day.pred_opt().unwrap();
    }

    day
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

    std::panic::set_hook(Box::new(|info| {
        let _ = terminal::disable_raw_mode();
        eprintln!("Panic: {info}");
    }));

    app.run();
    disable_raw_mode().unwrap();
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

    fn get_naivetime(&mut self, prompt: impl AsRef<str>) -> Option<TimeSinceMidnight> {
        loop {
            let s = self.get_user_input(&prompt).unwrap();
            if s.is_empty() {
                return None;
            };

            if let Ok(time) = NaiveTime::parse_from_str(&s, "%H:%M") {
                let time = naive_to_timesincemidnight(time);
                return Some(time);
            }
        }
    }

    pub fn start() -> Self {
        let today = current_day();
        let day: Saved<Day> = Saved::load_or_create(today);
        day.write().slots_config.make_valid();
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
        let slots = self.selected_day.read().slots_config.clone();
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
                let slots = self.selected_day.read().slots_config.clone();
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

                self.selected_day
                    .write()
                    .slots_config
                    .over_ride(idx, selected_slot);
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
                    self.selected_day.write().slots_config.swap(idx, idx - 1);
                    self.cursor.up();
                }
            }
            Action::Downswap => {
                let slots = self.selected_day.read().slots_config.clone();
                if slots.is_empty() {
                    return ControlFlow::Continue(());
                };

                let idx = self.cursor.index.clamp(0, slots.len() - 1);

                self.selected_day.write().slots_config.swap(idx, idx + 1);
                self.cursor
                    .down(self.selected_day.read().slots_config.len());
            }
            Action::Begin => {
                let slots = self.selected_day.read().slots_config.clone();
                if slots.is_empty() {
                    return ControlFlow::Continue(());
                };

                let idx = self.cursor.index.clamp(0, slots.len() - 1);
                self.selected_day
                    .write()
                    .slots_config
                    .set_start(idx, current_time());
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
                }
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

    // Since mako doesn't support editing notifications in-place, nuke all notifs if last
    // one was less than 10 sec ago. This will avoid multiple notifs at same time
    // with the unfortunate side effect it will also remove other notifs from other processes.
    if update_timestamp() < std::time::Duration::from_secs(10) {
        let _ = std::process::Command::new("makoctl")
            .arg("dismiss")
            .output();
    }

    let s = format!("new task: {}", &slot.configured.name);
    let _ = Notification::new().summary(&s).id(6006).show();
}

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

static LAST_TIMESTAMP: OnceLock<AtomicU64> = OnceLock::new();

fn timestamp_store() -> &'static AtomicU64 {
    LAST_TIMESTAMP.get_or_init(|| AtomicU64::new(0))
}

fn current_unix_time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn update_timestamp() -> std::time::Duration {
    let now = current_unix_time();
    let prev = timestamp_store().swap(now, Ordering::SeqCst);
    std::time::Duration::from_secs(now - prev)
}

pub fn timed_input(timeout_secs: u64) -> Option<Event> {
    if event::poll(std::time::Duration::from_secs(timeout_secs)).ok()? {
        event::read().ok()
    } else {
        None
    }
}

fn clock_emoji(time: TimeSinceMidnight) -> char {
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

fn hour_and_minute(time: TimeSinceMidnight) -> (u32, u32) {
    let secs = time.num_seconds();
    let hours = secs / 3600;
    let rem = secs - (hours * 3600);
    let minutes = rem / 60;
    (hours as u32, minutes as u32)
}

fn format_dur(dur: Duration) -> String {
    let mins = dur.num_seconds() / 60;
    format!("{:>5}m", mins)
}

fn format_naive(time: TimeSinceMidnight) -> String {
    let (hours, minutes) = hour_and_minute(time);
    format!("{:02}:{:02}", hours, minutes)
}

#[derive(Serialize, Deserialize, Default)]
pub struct SlotDtos(Vec<SlotDto>);

impl Deref for SlotDtos {
    type Target = Vec<SlotDto>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl SlotDtos {
    pub fn insert(&mut self, index: usize, slot: SlotDto) {
        let mut inner = self.0.clone();
        if index >= inner.len() {
            inner.push(slot);
        } else {
            inner.insert(index, slot);
        }

        if Self::validate(&inner).is_ok() {
            self.0 = inner;
        }
    }

    pub fn remove(&mut self, idx: usize) {
        if idx >= self.0.len() {
            return;
        }

        let mut inner = self.0.clone();
        inner.remove(idx);

        if Self::validate(&inner).is_ok() {
            self.0 = inner;
        }
    }

    pub fn unset_start(&mut self, idx: usize) {
        if idx >= self.0.len() {
            return;
        }

        let mut inner = self.0.clone();
        inner[idx].config.start = None;

        if Self::validate(&inner).is_ok() {
            self.0 = inner;
        }
    }

    pub fn set_start(&mut self, idx: usize, start: TimeSinceMidnight) {
        if idx >= self.0.len() {
            return;
        }

        let mut inner = self.0.clone();
        inner[idx].config.start = Some(start);

        if Self::validate(&inner).is_ok() {
            self.0 = inner;
        }
    }

    pub fn swap(&mut self, i: usize, j: usize) {
        let len = self.0.len();

        if i >= len || j >= len {
            return;
        }

        let mut inner = self.0.clone();
        inner.swap(i, j);

        if Self::validate(&inner).is_ok() {
            self.0 = inner;
        }
    }

    pub fn over_ride(&mut self, index: usize, slot: SlotDto) {
        let mut inner = self.0.clone();
        if index >= inner.len() {
            return;
        } else {
            inner[index] = slot;
        }

        if Self::validate(&inner).is_ok() {
            self.0 = inner;
        }
    }

    pub fn make_valid(&mut self) {
        let mut last_start: Option<TimeSinceMidnight> = None;

        for slot in &mut self.0 {
            let valid_time = if let Some(t) = &slot.config.start {
                if let Some(prev_t) = &last_start {
                    if t < prev_t {
                        false
                    } else {
                        last_start = Some(*t);
                        true
                    }
                } else {
                    true
                }
            } else {
                false
            };

            if !valid_time {
                slot.config.start = None;
            }
        }
    }

    fn validate(slots: &Vec<SlotDto>) -> Result<(), ()> {
        let mut last_start: Option<TimeSinceMidnight> = None;

        for slot in slots {
            if let Some(t) = &slot.config.start {
                if let Some(prev_t) = &last_start {
                    if t < prev_t {
                        return Err(());
                    } else {
                        last_start = Some(*t);
                    }
                }
            }
        }

        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
struct Day {
    day: NaiveDate,
    slots_config: SlotDtos,
    #[serde(skip)]
    slot_result: SingletonCache<Vec<SlotDto>, Vec<SlotResult>>,
}

impl DefaultWithId for Day {
    fn default_with_id(id: Self::Key) -> Self {
        Self {
            day: id,
            slots_config: Default::default(),
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
