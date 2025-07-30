use chrono::{Duration, NaiveTime};
use nonempty::NonEmpty;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fmt::{Debug, Display};
use std::mem;
use uuid::Uuid;

type ActId = Uuid;

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
        let configs: NonEmpty<SlotDto> = match NonEmpty::from_vec(configs)  {
            Some(configs) => configs,
            None => return vec![],
        };
        let slotblocks = get_slotblocks(start_time, total_time, configs);
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
    slots: NonEmpty<SlotDto>,
    end_time: NaiveTime,
}

impl SlotBlock {
    fn new(start: NaiveTime, slots: NonEmpty<SlotDto>, end_time: NaiveTime) -> Self {
        assert!(end_time >= start);

        Self {
            start,
            slots,
            end_time,
        }
    }

    fn get_allocated(&self) -> SlotAllocTime {
        assert!(self.end_time > self.start);

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

fn append_blocks(start_time: NaiveTime, blocks: &mut NonEmpty<SlotBlock>, dtos: NonEmpty<SlotDto>) {

}

fn get_slotblocks(
    start_time: NaiveTime,
    total_time: Duration,
    configs: NonEmpty<SlotDto>,
) -> NonEmpty<SlotBlock> {
    let mut blocks: Vec<SlotBlock> = vec![];

    let mut buf: Vec<SlotDto> = vec![];
    let mut configs: VecDeque<SlotDto> = configs.into_iter().collect();

    while let Some(config) = configs.pop_front() {
        if let Some(start) = config.config.start {
            if let Some(buf) =  NonEmpty::from_vec(mem::take(&mut buf)) {
                let start_time = match blocks.last() {
                    Some(block) => block.end_time,
                    None => start_time,
                };

                let block = SlotBlock::new(start_time, buf, start);

                blocks.push(block);
            }
        }

        buf.push(config);
    }

    if let Some(buf) =  NonEmpty::from_vec(mem::take(&mut buf)) {
        let block_start_time = match blocks.last() {
            Some(block) => block.end_time,
            None => start_time,
        };

        let block = SlotBlock::new(
            block_start_time,
            buf,
            start_time + total_time,
        );

        blocks.push(block);

    }


    NonEmpty::from_vec(blocks).unwrap()
}

pub fn t(h: u32, m: u32) -> NaiveTime {
    NaiveTime::from_hms_opt(h, m, 0).unwrap()
}

pub fn dur(mins: i64) -> Duration {
    Duration::minutes(mins)
}
