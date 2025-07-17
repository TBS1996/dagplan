use chrono::{Duration, NaiveDate, NaiveTime, Utc};
use crossterm::cursor::MoveTo;
use crossterm::execute;
use crossterm::terminal::{Clear, ClearType};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::fmt::{write, Debug, Display};
use std::io::{self, Write};
use std::mem;
use std::path::PathBuf;
use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::{fs, sync::Arc};
use uuid::Uuid;
use vedvaring::{DefaultWithId, FsTrait, Saved};

type ActId = Uuid;
type SlotId = Uuid;

pub fn calculate_slots(
    start_time: NaiveTime,
    total_time: Duration,
    configs: Vec<SlotDto>,
) -> Vec<SlotResult> {
    let start_time = configs
        .first()
        .and_then(|x| x.config.start)
        .unwrap_or(start_time);
    TimeSlotConfig::calculate_slots(start_time, total_time, configs)
}

#[derive(Clone, Serialize, Deserialize, Debug, Hash, Eq, PartialEq)]
pub struct SlotDto {
    pub name: String,
    pub act: Option<ActId>,
    pub config: TimeSlotConfig,
}

impl Default for SlotDto {
    fn default() -> Self {
        Self {
            name: format!("..."),
            act: Default::default(),
            config: Default::default(),
        }
    }
}

#[derive(PartialEq, Eq, Clone, Debug)]
pub enum ScheduleError {
    NoElasticSlots,
    InsufficientFixedTime,
}

/// The configuration for when a slot should be. Doesn't mean it will be on that time that depends on its constraints
#[derive(Clone, Serialize, Deserialize, Debug, Hash, Eq, PartialEq)]
pub struct TimeSlotConfig {
    pub start: Option<NaiveTime>,
    pub length: Duration,
    pub fixed_length: bool,
}

impl Default for TimeSlotConfig {
    fn default() -> Self {
        Self {
            start: Default::default(),
            length: Duration::hours(1),
            fixed_length: Default::default(),
        }
    }
}

impl TimeSlotConfig {
    pub fn calculate_slots(
        start_time: NaiveTime,
        total_time: Duration,
        configs: Vec<SlotDto>,
    ) -> Vec<SlotResult> {
        let slotblocks = get_slotblocks(start_time, total_time, configs);
        dbg!(&slotblocks);
        let mut out: Vec<SlotResult> = vec![];

        for block in slotblocks {
            dbg!();
            dbg!(&block);
            let res = block.get_slot_result();
            dbg!(&res);
            out.extend(res);
        }

        out
    }
}

use humantime;

/// The calculated start and length time of a slot after having to fit within constraints
#[derive(PartialEq, Eq, Clone)]
pub struct SlotResult {
    pub start: NaiveTime,
    pub length: Duration,
    pub warning: Result<(), ScheduleError>,
    pub configured: SlotDto,
}

impl Display for SlotResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let length = humantime::format_duration(self.length.to_std().unwrap());
        let req_length =
            humantime::format_duration(self.configured.config.length.to_std().unwrap());
        let s = format!(
            "name: {}, start: {}, length: {}, requested length: {}, res: {:?}",
            self.configured.name.as_str(),
            self.start,
            length,
            req_length,
            self.warning
        );

        write!(f, "{s}")
    }
}

impl Debug for SlotResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = self.to_string();
        write!(f, "{s}")
    }
}

struct SlotAllocTime {
    /// Total time allocated to the block. All slots summed up should fit this.
    tot_alloc: Duration,
    /// The sum of all the fixed lengths in a block
    tot_req_fixed: Duration,
    /// The sum of all the requested elastic time in a block.
    tot_req_elastic: Duration,
    /// The allocated space for elastic slots to expand/shrink into
    elastic_alloc_time: Duration,
}

impl Debug for SlotAllocTime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let tot_alloc = humantime::format_duration(self.tot_alloc.to_std().unwrap());
        let tot_req_fixed = humantime::format_duration(self.tot_req_fixed.to_std().unwrap());
        let tot_req_elastic = humantime::format_duration(self.tot_req_elastic.to_std().unwrap());
        let elastic_alloc_time =
            humantime::format_duration(self.elastic_alloc_time.to_std().unwrap_or_default());

        let s = format!("tot alloc {tot_alloc}, tot req fixed: {tot_req_fixed}, tot_req_elastic: {tot_req_elastic}, elastic alloc time: {elastic_alloc_time} ");
        write!(f, "{s}")
    }
}

impl SlotAllocTime {
    /// The ratio to change length of 'fixed time' slots. Wll only be 'some' if some config error
    ///
    // fixed lengths shouldn't have a scaling ratio generally, but it hsa to have in two possibilities.
    // 1. If the total amount of fixed length is greater than the allocated time ,we gotta shrink it
    // 2. if no elastic slots, it probably has to expand unless its exactly the same length as allocated itme.
    fn fixed_ratio(&self) -> Option<(f32, ScheduleError)> {
        if self.tot_req_fixed.is_zero() {
            return None;
        };

        let no_elastic_slots = self.tot_req_elastic.is_zero();
        let too_little_time_alloc = self.tot_req_fixed > self.tot_alloc;

        if no_elastic_slots || too_little_time_alloc {
            let ratio =
                self.tot_alloc.num_seconds() as f32 / self.tot_req_fixed.num_seconds() as f32;
            let err = if too_little_time_alloc {
                ScheduleError::InsufficientFixedTime
            } else {
                ScheduleError::NoElasticSlots
            };

            Some((ratio, err))
        } else {
            None
        }
    }

    /// How much the elastic slots should be modified
    fn elastic_ratio(&self) -> f32 {
        if self.fixed_ratio().is_some() {
            0.
        } else {
            self.elastic_alloc_time.num_seconds() as f32 / self.tot_req_elastic.num_seconds() as f32
        }
    }
}

/// Represents a bunch of slots between two fixed start times
///
/// invariants: start < end_time
/// !slots.is_empty()
#[derive(Debug)]
struct SlotBlock {
    start: NaiveTime,
    /// Vector of slot length and if the slot length should be fixed
    slots: Vec<SlotDto>,
    end_time: NaiveTime,
}

impl SlotBlock {
    fn new(start: NaiveTime, slots: Vec<SlotDto>, end_time: NaiveTime) -> Self {
        debug_assert!(end_time > start);

        Self {
            start,
            slots,
            end_time,
        }
    }

    fn get_allocated(&self) -> SlotAllocTime {
        debug_assert!(self.end_time > self.start);
        debug_assert!(!self.slots.is_empty());

        let tot_alloc = self.end_time - self.start;
        let tot_req_fixed: Duration = self
            .slots
            .iter()
            .map(|slot| {
                slot.config
                    .fixed_length
                    .then_some(slot.config.length)
                    .unwrap_or_default()
            })
            .sum();
        let tot_req_elastic: Duration = self
            .slots
            .iter()
            .map(|slot| {
                (!slot.config.fixed_length)
                    .then_some(slot.config.length)
                    .unwrap_or_default()
            })
            .sum();
        let elastic_alloc_time = tot_alloc.checked_sub(&tot_req_fixed).unwrap_or_default();

        SlotAllocTime {
            tot_alloc,
            tot_req_fixed,
            tot_req_elastic,
            elastic_alloc_time,
        }
    }

    fn get_slot_result(self) -> Vec<SlotResult> {
        let mut out: Vec<SlotResult> = vec![];

        let alloc = self.get_allocated();

        let (fixed_ratio, fix_warn) = alloc
            .fixed_ratio()
            .map(|(ratio, warn)| (ratio, Err(warn)))
            .unwrap_or((1.0, Ok(())));
        let elastic_ratio = alloc.elastic_ratio();

        dbg!(&alloc, fixed_ratio, &fix_warn, elastic_ratio);

        let mut start = self.start;

        for slot in self.slots {
            let fixed = slot.config.fixed_length;
            let length = slot.config.length.num_seconds() as f32
                * if fixed { fixed_ratio } else { elastic_ratio };

            let slot = SlotResult {
                start,
                length: Duration::from_std(std::time::Duration::from_secs_f32(length)).unwrap(),
                warning: if fixed { fix_warn.clone() } else { Ok(()) },
                configured: slot,
            };

            start = start + slot.length;
            out.push(slot);
        }

        out
    }
}

fn get_slotblocks(
    start_time: NaiveTime,
    total_time: Duration,
    configs: Vec<SlotDto>,
) -> Vec<SlotBlock> {
    let mut blocks: Vec<SlotBlock> = vec![];
    let mut buf: Vec<SlotDto> = vec![];
    let mut configs: VecDeque<SlotDto> = configs.into_iter().collect();

    while let Some(config) = configs.pop_front() {
        if let Some(start) = config.config.start {
            if !buf.is_empty() {
                let start_time = match blocks.last() {
                    Some(block) => block.end_time,
                    None => start_time,
                };

                let block = SlotBlock::new(start_time, mem::take(&mut buf), start);

                blocks.push(block);
            }
        }

        buf.push(config);
    }

    if !buf.is_empty() {
        let block_start_time = match blocks.last() {
            Some(block) => block.end_time,
            None => start_time,
        };

        let block = SlotBlock::new(
            block_start_time,
            mem::take(&mut buf),
            start_time + total_time,
        );

        blocks.push(block);
    }

    blocks
}

pub fn t(h: u32, m: u32) -> NaiveTime {
    NaiveTime::from_hms_opt(h, m, 0).unwrap()
}

pub fn dur(mins: i64) -> Duration {
    Duration::minutes(mins)
}

/*
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, NaiveTime};

    fn slot(start: Option<NaiveTime>, length: i64, fixed: bool) -> TimeSlotConfig {
        TimeSlotConfig {
            start,
            length: dur(length),
            fixed_length: fixed,
        }
    }

    #[test]
    fn calc_slots() {
        let start_time = t(7, 0);
        let total_time = dur(16 * 60);
        let configs = vec![
            slot(None, 10, false),
            slot(None, 20, false),
            slot(Some(t(10, 45)), 15, true),
            slot(Some(t(11, 15)), 15, false),
            slot(None, 20, false),
            slot(None, 20, false),
        ];

        let res = TimeSlotConfig::calculate_slots(start_time, total_time, configs);
        dbg!(res);
    }

    #[test]
    fn test_no_fixed_starts() {
        let start_time = t(8, 0);
        let total_time = dur(60);
        let configs = vec![slot(None, 10, false), slot(None, 20, false)];

        let blocks = get_slotblocks(start_time, total_time, configs);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].slots.len(), 2);
        assert_eq!(blocks[0].end_time, t(9, 0));
    }

    #[test]
    fn test_single_fixed_start() {
        let start_time = t(8, 0);
        let total_time = dur(90);
        let configs = vec![
            slot(None, 10, false),          // should fill up entire first hour
            slot(Some(t(9, 0)), 10, false), // should start at 9, and be half the length as the next
        ];

        let blocks = get_slotblocks(start_time, total_time, configs);

        assert_eq!(blocks.len(), 2);

        // First block ends at 09:00
        assert_eq!(blocks[0].end_time, t(9, 0));
        assert_eq!(blocks[0].slots.len(), 1); // only the first slot

        // Second block ends at 09:30 because it fills the entire allocated time
        assert_eq!(blocks[1].end_time, t(9, 30));
        assert_eq!(blocks[1].slots.len(), 1);
    }

    #[test]
    fn test_multiple_fixed_starts() {
        let start_time = t(8, 0);
        let total_time = dur(180);
        let configs = vec![
            slot(None, 10, false),
            slot(Some(t(9, 0)), 15, true),
            slot(Some(t(10, 0)), 20, true),
        ];

        let blocks = get_slotblocks(start_time, total_time, configs);

        assert_eq!(blocks.len(), 3);

        assert_eq!(blocks[0].slots.len(), 1); // first slot before fixed time
        assert_eq!(blocks[0].end_time, t(9, 0));

        assert_eq!(blocks[1].slots.len(), 1);
        assert_eq!(blocks[1].end_time, t(10, 0)); // 09:00 + 15

        assert_eq!(blocks[2].slots.len(), 1);
        assert_eq!(blocks[2].end_time, start_time + total_time); // 10:00 + 20
    }

    #[test]
    fn test_only_fixed_starts() {
        let start_time = t(7, 0);
        let total_time = dur(120);
        let configs = vec![slot(Some(t(8, 0)), 15, true), slot(Some(t(9, 0)), 20, true)];

        let blocks = get_slotblocks(start_time, total_time, configs);

        dbg!(&blocks);

        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].end_time, t(9, 0));
        assert_eq!(blocks[1].end_time, start_time + total_time);
    }

    #[test]
    fn test_empty_config() {
        let blocks = get_slotblocks(t(8, 0), dur(60), vec![]);
        assert_eq!(blocks.len(), 0);
    }
}

*/
