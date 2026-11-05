/*
 * @spec-handoff
 * @interface evaluate_schedule(ScheduleInput): Result<ScheduleEvaluation, String>
 * @behavior
 *   - Evaluates selected start-day-owned windows at a supplied UTC instant.
 *   - Returns current membership and the earliest later occurrence using Clock rules.
 *   - Resolves gaps forward, start folds earlier, and end folds later.
 * @edge-cases
 *   - Disabled, empty-day, equal-time, invalid, and collapsed windows are inactive.
 *   - Resolution stops with an error after 48 hours without a valid local minute.
 */
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;
    use chrono::{Duration, LocalResult};

    #[derive(Clone, Copy)]
    struct OffsetRange {
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        offset_seconds: i32,
    }

    struct TestClock {
        ranges: Vec<OffsetRange>,
    }

    impl TestClock {
        fn new(ranges: Vec<OffsetRange>) -> Self {
            Self { ranges }
        }

        fn utc(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
            Utc.with_ymd_and_hms(year, month, day, hour, minute, 0)
                .single()
                .expect("valid UTC fixture")
        }

        fn offset(seconds: i32) -> FixedOffset {
            FixedOffset::east_opt(seconds).expect("valid fixed-offset fixture")
        }

        fn covering(&self, instant: DateTime<Utc>) -> OffsetRange {
            self.ranges
                .iter()
                .copied()
                .find(|range| range.start <= instant && instant < range.end)
                .expect("fixture covers tested instant")
        }
    }

    impl Clock for TestClock {
        fn now(&self) -> DateTime<Utc> {
            self.ranges[0].start
        }

        fn to_local(&self, instant: DateTime<Utc>) -> DateTime<FixedOffset> {
            instant.with_timezone(&Self::offset(self.covering(instant).offset_seconds))
        }

        fn resolve_local(&self, local: NaiveDateTime) -> LocalResult<DateTime<Utc>> {
            let mut candidates = self
                .ranges
                .iter()
                .filter_map(|range| {
                    let instant = local
                        .and_utc()
                        .checked_sub_signed(Duration::seconds(i64::from(range.offset_seconds)))?;
                    (range.start <= instant && instant < range.end).then_some(instant)
                })
                .collect::<Vec<_>>();
            candidates.sort();
            candidates.dedup();
            match candidates.as_slice() {
                [] => LocalResult::None,
                [only] => LocalResult::Single(*only),
                [first, second] => LocalResult::Ambiguous(*second, *first),
                _ => panic!("a fixed-offset fixture has at most two local instants"),
            }
        }
    }

    struct UnresolvableClock;

    impl Clock for UnresolvableClock {
        fn now(&self) -> DateTime<Utc> {
            TestClock::utc(2026, 3, 2, 9, 0)
        }

        fn to_local(&self, instant: DateTime<Utc>) -> DateTime<FixedOffset> {
            instant.with_timezone(&FixedOffset::east_opt(0).expect("UTC offset fixture"))
        }

        fn resolve_local(&self, _: NaiveDateTime) -> LocalResult<DateTime<Utc>> {
            LocalResult::None
        }
    }

    fn utc_clock() -> TestClock {
        TestClock::new(vec![OffsetRange {
            start: TestClock::utc(2026, 1, 1, 0, 0),
            end: TestClock::utc(2027, 1, 1, 0, 0),
            offset_seconds: 0,
        }])
    }

    fn schedule(enabled: bool, start: (u32, u32), end: (u32, u32), days: Vec<Weekday>) -> Schedule {
        Schedule {
            enabled,
            start: NaiveTime::from_hms_opt(start.0, start.1, 0).expect("valid start fixture"),
            end: NaiveTime::from_hms_opt(end.0, end.1, 0).expect("valid end fixture"),
            days,
        }
    }

    fn evaluate<'a>(
        schedule: &'a Schedule,
        at: DateTime<Utc>,
        clock: &'a dyn Clock,
    ) -> Result<ScheduleEvaluation, String> {
        evaluate_schedule(ScheduleInput {
            schedule,
            at,
            clock,
        })
    }

    #[test]
    fn evaluates_start_inclusive_end_exclusive_windows_and_ignores_empty_or_equal_schedules() {
        let clock = utc_clock();
        let weekday = schedule(true, (9, 0), (17, 0), vec![Weekday::Mon, Weekday::Wed]);
        assert!(evaluate(&weekday, TestClock::utc(2026, 3, 2, 9, 0), &clock)
            .expect("evaluation succeeds")
            .in_window());
        assert!(
            !evaluate(&weekday, TestClock::utc(2026, 3, 2, 17, 0), &clock)
                .expect("evaluation succeeds")
                .in_window()
        );
        assert!(evaluate(&weekday, TestClock::utc(2026, 3, 4, 9, 0), &clock)
            .expect("evaluation succeeds")
            .in_window());
        let empty = schedule(true, (9, 0), (17, 0), vec![]);
        assert!(evaluate(&empty, TestClock::utc(2026, 3, 2, 10, 0), &clock)
            .expect("empty days are valid")
            .current
            .is_none());
        let equal = schedule(true, (9, 0), (9, 0), vec![Weekday::Mon]);
        assert!(evaluate(&equal, TestClock::utc(2026, 3, 2, 10, 0), &clock)
            .expect("equal bounds are valid")
            .next
            .is_none());
    }

    #[test]
    fn disabled_schedule_has_no_restriction_membership_or_next_occurrence() {
        let clock = utc_clock();
        let disabled = schedule(false, (9, 0), (17, 0), vec![Weekday::Mon]);
        let evaluation = evaluate(&disabled, TestClock::utc(2026, 3, 2, 10, 0), &clock)
            .expect("disabled schedule evaluates");
        assert!(!evaluation.in_window());
        assert_eq!(evaluation.next, None);
    }

    #[test]
    fn parses_weekday_tokens_case_insensitively_and_rejects_invalid_times() {
        let mut config = AppConfig {
            schedule_enabled: true,
            schedule_days: vec!["MON".into(), "wed".into(), "Mon".into(), "noday".into()],
            ..AppConfig::default()
        };
        let parsed = Schedule::from_config(&config).expect("valid fields parse");
        assert_eq!(parsed.days, vec![Weekday::Mon, Weekday::Wed]);
        config.schedule_start_hour = 24;
        assert!(Schedule::from_config(&config).is_err());
        config.schedule_start_hour = 9;
        config.schedule_end_minute = 60;
        assert!(Schedule::from_config(&config).is_err());
    }

    #[test]
    fn overnight_occurrences_are_owned_by_their_selected_start_day_across_week_rollover() {
        let clock = utc_clock();
        let monday = schedule(true, (22, 0), (6, 0), vec![Weekday::Mon]);
        assert!(evaluate(&monday, TestClock::utc(2026, 3, 3, 5, 59), &clock)
            .expect("evaluation succeeds")
            .in_window());
        assert!(
            !evaluate(&monday, TestClock::utc(2026, 3, 2, 5, 59), &clock)
                .expect("evaluation succeeds")
                .in_window()
        );
        let sunday = schedule(true, (22, 0), (6, 0), vec![Weekday::Sun]);
        assert!(evaluate(&sunday, TestClock::utc(2026, 3, 2, 5, 59), &clock)
            .expect("evaluation succeeds")
            .in_window());
    }

    #[test]
    fn returns_the_next_selected_start_after_current_occurrence_ends_and_none_for_empty_days() {
        let clock = utc_clock();
        let selected = schedule(true, (9, 0), (17, 0), vec![Weekday::Mon, Weekday::Wed]);
        let evaluation = evaluate(&selected, TestClock::utc(2026, 3, 2, 17, 0), &clock)
            .expect("evaluation succeeds");
        assert_eq!(evaluation.current, None);
        assert_eq!(
            evaluation.next.expect("Wednesday is upcoming").start,
            TestClock::utc(2026, 3, 4, 9, 0)
        );
        let inside = evaluate(&selected, TestClock::utc(2026, 3, 2, 10, 0), &clock)
            .expect("evaluation succeeds");
        assert_eq!(
            inside.next.expect("next means later than current").start,
            TestClock::utc(2026, 3, 4, 9, 0)
        );
        let at_start = evaluate(&selected, TestClock::utc(2026, 3, 2, 9, 0), &clock)
            .expect("evaluation succeeds");
        assert_eq!(
            at_start.next.expect("current occurrence is not next").start,
            TestClock::utc(2026, 3, 4, 9, 0)
        );
        let none = schedule(true, (9, 0), (17, 0), vec![]);
        assert_eq!(
            evaluate(&none, TestClock::utc(2026, 3, 2, 17, 0), &clock)
                .expect("empty days are valid")
                .next,
            None
        );
    }

    #[test]
    fn resolves_spring_gap_boundaries_to_the_first_valid_local_minute() {
        let transition = TestClock::utc(2026, 3, 29, 1, 0);
        let clock = TestClock::new(vec![
            OffsetRange {
                start: TestClock::utc(2026, 1, 1, 0, 0),
                end: transition,
                offset_seconds: 3600,
            },
            OffsetRange {
                start: transition,
                end: TestClock::utc(2027, 1, 1, 0, 0),
                offset_seconds: 7200,
            },
        ]);
        let start = NaiveDate::from_ymd_opt(2026, 3, 29)
            .unwrap()
            .and_hms_opt(2, 30, 0)
            .unwrap();
        let end = NaiveDate::from_ymd_opt(2026, 3, 29)
            .unwrap()
            .and_hms_opt(3, 30, 0)
            .unwrap();
        assert_eq!(
            resolve_boundary(BoundaryInput {
                local: start,
                edge: BoundaryEdge::Start,
                clock: &clock
            })
            .expect("gap start resolves"),
            transition
        );
        assert_eq!(
            resolve_boundary(BoundaryInput {
                local: end,
                edge: BoundaryEdge::End,
                clock: &clock
            })
            .expect("post-gap end resolves"),
            TestClock::utc(2026, 3, 29, 1, 30)
        );
    }

    #[test]
    fn resolves_fall_fold_start_earlier_end_later_and_keeps_one_occurrence_identity() {
        let transition = TestClock::utc(2026, 10, 25, 1, 0);
        let clock = TestClock::new(vec![
            OffsetRange {
                start: TestClock::utc(2026, 1, 1, 0, 0),
                end: transition,
                offset_seconds: 7200,
            },
            OffsetRange {
                start: transition,
                end: TestClock::utc(2027, 1, 1, 0, 0),
                offset_seconds: 3600,
            },
        ]);
        let day = NaiveDate::from_ymd_opt(2026, 10, 25).unwrap();
        let folded = day.and_hms_opt(2, 30, 0).unwrap();
        assert_eq!(
            resolve_boundary(BoundaryInput {
                local: folded,
                edge: BoundaryEdge::Start,
                clock: &clock
            })
            .expect("folded start resolves"),
            TestClock::utc(2026, 10, 25, 0, 30)
        );
        assert_eq!(
            resolve_boundary(BoundaryInput {
                local: folded,
                edge: BoundaryEdge::End,
                clock: &clock
            })
            .expect("folded end resolves"),
            TestClock::utc(2026, 10, 25, 1, 30)
        );
        let window = schedule(true, (2, 30), (3, 0), vec![Weekday::Sun]);
        let first = evaluate(&window, TestClock::utc(2026, 10, 25, 0, 45), &clock)
            .expect("first repeated hour evaluates")
            .current
            .expect("first repeated hour is inside");
        let second = evaluate(&window, TestClock::utc(2026, 10, 25, 1, 15), &clock)
            .expect("second repeated hour evaluates")
            .current
            .expect("second repeated hour is inside");
        assert_eq!(first, second);
        let next_day = schedule(true, (2, 30), (3, 0), vec![Weekday::Sun, Weekday::Mon]);
        let monday = evaluate(&next_day, TestClock::utc(2026, 10, 26, 1, 45), &clock)
            .expect("next day evaluates")
            .current
            .expect("next day is inside");
        assert_ne!(first, monday);
        assert_eq!(
            monday.start_day,
            NaiveDate::from_ymd_opt(2026, 10, 26).unwrap()
        );
    }

    #[test]
    fn rejects_occurrences_collapsed_by_boundary_resolution_and_errors_after_48_hours() {
        let transition = TestClock::utc(2026, 3, 29, 1, 0);
        let gap_clock = TestClock::new(vec![
            OffsetRange {
                start: TestClock::utc(2026, 1, 1, 0, 0),
                end: transition,
                offset_seconds: 3600,
            },
            OffsetRange {
                start: transition,
                end: TestClock::utc(2027, 1, 1, 0, 0),
                offset_seconds: 7200,
            },
        ]);
        let collapsed = schedule(true, (2, 30), (3, 0), vec![Weekday::Sun]);
        assert!(
            !evaluate(&collapsed, TestClock::utc(2026, 3, 29, 1, 0), &gap_clock)
                .expect("collapsed occurrence is ignored")
                .in_window()
        );
        let no_zone = UnresolvableClock;
        let local = NaiveDate::from_ymd_opt(2026, 3, 2)
            .unwrap()
            .and_hms_opt(9, 0, 0)
            .unwrap();
        assert!(resolve_boundary(BoundaryInput {
            local,
            edge: BoundaryEdge::Start,
            clock: &no_zone
        })
        .is_err());
        assert!(evaluate(
            &schedule(true, (9, 0), (17, 0), vec![Weekday::Mon]),
            TestClock::utc(2026, 3, 2, 9, 0),
            &no_zone
        )
        .is_err());
    }

    #[test]
    fn reevaluates_membership_when_timezone_offset_changes_and_never_replays_missed_windows() {
        let shift = TestClock::utc(2026, 3, 2, 9, 0);
        let clock = TestClock::new(vec![
            OffsetRange {
                start: TestClock::utc(2026, 1, 1, 0, 0),
                end: shift,
                offset_seconds: 3600,
            },
            OffsetRange {
                start: shift,
                end: TestClock::utc(2027, 1, 1, 0, 0),
                offset_seconds: 7200,
            },
        ]);
        let window = schedule(true, (9, 0), (10, 0), vec![Weekday::Mon]);
        assert!(evaluate(&window, TestClock::utc(2026, 3, 2, 8, 0), &clock)
            .expect("before offset change evaluates")
            .in_window());
        let after_change = evaluate(&window, TestClock::utc(2026, 3, 2, 9, 0), &clock)
            .expect("after offset change evaluates");
        assert!(!after_change.in_window());
        assert_eq!(
            after_change.next.expect("next week is upcoming").start,
            TestClock::utc(2026, 3, 9, 7, 0)
        );
    }
}
