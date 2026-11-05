#![allow(dead_code)] // skeleton: wired in E3

use crate::config::AppConfig;
use chrono::{
    DateTime, FixedOffset, Local, LocalResult, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc,
    Weekday,
};

pub(crate) trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
    fn to_local(&self, instant: DateTime<Utc>) -> DateTime<FixedOffset>;
    fn resolve_local(&self, local: NaiveDateTime) -> LocalResult<DateTime<Utc>>;
}

#[derive(Debug, Default)]
pub(crate) struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }

    fn to_local(&self, instant: DateTime<Utc>) -> DateTime<FixedOffset> {
        instant.with_timezone(&Local).fixed_offset()
    }

    fn resolve_local(&self, local: NaiveDateTime) -> LocalResult<DateTime<Utc>> {
        Local
            .from_local_datetime(&local)
            .map(|value| value.with_timezone(&Utc))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Schedule {
    pub(crate) enabled: bool,
    pub(crate) start: NaiveTime,
    pub(crate) end: NaiveTime,
    pub(crate) days: Vec<Weekday>,
}

impl Schedule {
    pub(crate) fn from_config(_config: &AppConfig) -> Result<Self, String> {
        Ok(Self {
            enabled: false,
            start: NaiveTime::MIN,
            end: NaiveTime::MIN,
            days: Vec::new(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Occurrence {
    pub(crate) start_day: NaiveDate,
    pub(crate) start: DateTime<Utc>,
    pub(crate) end: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScheduleEvaluation {
    pub(crate) at: DateTime<Utc>,
    pub(crate) current: Option<Occurrence>,
    pub(crate) next: Option<Occurrence>,
}

impl ScheduleEvaluation {
    pub(crate) fn in_window(&self) -> bool {
        self.current.is_some()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BoundaryEdge {
    Start,
    End,
}

pub(crate) struct BoundaryInput<'a> {
    pub(crate) local: NaiveDateTime,
    pub(crate) edge: BoundaryEdge,
    pub(crate) clock: &'a dyn Clock,
}

pub(crate) fn resolve_boundary(_input: BoundaryInput<'_>) -> Result<DateTime<Utc>, String> {
    Err("schedule boundary skeleton".to_owned())
}

pub(crate) struct ScheduleInput<'a> {
    pub(crate) schedule: &'a Schedule,
    pub(crate) at: DateTime<Utc>,
    pub(crate) clock: &'a dyn Clock,
}

pub(crate) fn evaluate_schedule(input: ScheduleInput<'_>) -> Result<ScheduleEvaluation, String> {
    Ok(ScheduleEvaluation {
        at: input.at,
        current: None,
        next: None,
    })
}
