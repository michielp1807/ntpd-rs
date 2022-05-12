use crate::{
    filter::{FilterTuple, LastMeasurements},
    packet::{NtpAssociationMode, NtpLeapIndicator},
    time_types::{FrequencyTolerance, NtpInstant},
    NtpDuration, NtpHeader, NtpTimestamp, PollInterval, ReferenceId,
};

const MAX_STRATUM: u8 = 16;

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct PeerStatistics {
    pub offset: NtpDuration,
    pub delay: NtpDuration,

    pub dispersion: NtpDuration,
    pub jitter: f64,
}

#[derive(Debug, Clone)]
pub struct Peer {
    // Poll interval state
    last_poll_interval: PollInterval,
    next_poll_interval: PollInterval,
    remote_min_poll_interval: PollInterval,

    // Last packet information
    next_expected_origin: Option<NtpTimestamp>,

    statistics: PeerStatistics,
    last_measurements: LastMeasurements,
    last_packet: NtpHeader,
    time: NtpInstant,
    #[allow(dead_code)]
    peer_id: ReferenceId,
    our_id: ReferenceId,
    reach: Reach,
}

/// Used to determine whether the server is reachable and the data are fresh
///
/// This value is represented as an 8-bit shift register. The register is shifted left
/// by one bit when a packet is sent and the rightmost bit is set to zero.  
/// As valid packets arrive, the rightmost bit is set to one.
/// If the register contains any nonzero bits, the server is considered reachable;
/// otherwise, it is unreachable.
#[derive(Debug, Default, Clone, Copy)]
struct Reach(u8);

impl Reach {
    fn is_reachable(&self) -> bool {
        self.0 != 0
    }

    /// We have just received a packet, so the peer is definitely reachable
    fn received_packet(&mut self) {
        self.0 |= 1;
    }

    /// A packet received some number of poll intervals ago is decreasingly relevant for
    /// determining that a peer is still reachable. We discount the packets received so far.
    fn poll(&mut self) {
        self.0 <<= 1
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SystemSnapshot {
    /// May be updated by local_clock
    pub poll_interval: PollInterval,
    /// A constant at runtime
    pub precision: NtpDuration,
    /// May be updated by clock_update
    pub leap_indicator: NtpLeapIndicator,
}

impl Default for SystemSnapshot {
    fn default() -> Self {
        Self {
            poll_interval: PollInterval::default(),
            precision: NtpDuration::from_exponent(-18),
            leap_indicator: NtpLeapIndicator::Unknown,
        }
    }
}

pub enum IgnoreReason {
    /// The association mode is not one that this peer supports
    InvalidMode,
    /// The send time on the received packet is not the time we sent it at
    InvalidPacketTime,
    /// Received a Kiss-o'-Death https://datatracker.ietf.org/doc/html/rfc5905#section-7.4
    KissIgnore,
    /// Received a DENY or RSTR Kiss-o'-Death, and must demobilize the association
    KissDemobilize,
    /// The best packet is older than the peer's current time
    TooOld,
}

#[derive(Debug, Clone, Copy)]
pub struct PeerSnapshot {
    pub(crate) time: NtpInstant,
    pub(crate) root_distance_without_time: NtpDuration,
    pub(crate) stratum: u8,
    pub(crate) statistics: PeerStatistics,
}

impl PeerSnapshot {
    pub(crate) fn accept_synchronization(
        &self,
        local_clock_time: NtpInstant,
        frequency_tolerance: FrequencyTolerance,
        distance_threshold: NtpDuration,
        system_poll: PollInterval,
    ) -> Result<(), AcceptSynchronizationError> {
        use AcceptSynchronizationError::*;

        // the only check that is time-dependent is the distance check. All other checks are
        // handled by the peer, an no PeerUpdated would be produced if any of those checks fails

        //  A distance error occurs if the root distance exceeds the
        //  distance threshold plus an increment equal to one poll interval.
        let distance = self.root_distance(local_clock_time, frequency_tolerance);
        if distance > distance_threshold + (system_poll.as_duration() * frequency_tolerance) {
            return Err(Distance);
        }

        Ok(())
    }

    pub(crate) fn root_distance(
        &self,
        local_clock_time: NtpInstant,
        frequency_tolerance: FrequencyTolerance,
    ) -> NtpDuration {
        self.root_distance_without_time + ((local_clock_time - self.time) * frequency_tolerance)
    }
}

#[derive(Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum AcceptSynchronizationError {
    ServerUnreachable,
    Loop,
    Distance,
    Stratum,
}

impl Peer {
    pub fn new(our_id: ReferenceId, peer_id: ReferenceId, local_clock_time: NtpInstant) -> Self {
        // we initialize with the current time so that we're in the correct epoch.
        let time = local_clock_time;

        Self {
            last_poll_interval: PollInterval::MIN,
            next_poll_interval: PollInterval::MIN,
            remote_min_poll_interval: PollInterval::MIN,

            next_expected_origin: None,

            statistics: Default::default(),
            last_measurements: Default::default(),
            last_packet: Default::default(),
            time,
            our_id,
            peer_id,
            reach: Default::default(),
        }
    }

    pub fn get_interval_next_poll(&mut self, system_poll_interval: PollInterval) -> PollInterval {
        self.last_poll_interval = system_poll_interval
            .max(self.remote_min_poll_interval)
            .max(self.next_poll_interval);

        self.next_poll_interval = self.last_poll_interval.inc();

        self.last_poll_interval
    }

    pub fn generate_poll_message(&mut self, local_clock_time: NtpInstant) -> NtpHeader {
        self.reach.poll();

        let mut packet = NtpHeader::new();
        packet.poll = self.last_poll_interval.as_log();
        packet.mode = NtpAssociationMode::Client;

        // we write into the origin_timestamp and transmit_timestamp to validate the packet we get
        // back. The origin_timestamp must not be changed, the transmit_timestamp must be changed
        let local_clock_time = NtpTimestamp::from_bits(local_clock_time.to_bits());
        self.next_expected_origin = Some(local_clock_time);

        // in handle_incoming, we check that this field is unchanged
        packet.origin_timestamp = local_clock_time;

        // in handle_incoming, we check that this field was updated
        packet.transmit_timestamp = local_clock_time;

        packet
    }

    pub fn handle_incoming(
        &mut self,
        system: SystemSnapshot,
        message: NtpHeader,
        local_clock_time: NtpInstant,
        frequency_tolerance: FrequencyTolerance,
        recv_time: NtpTimestamp,
    ) -> Result<PeerSnapshot, IgnoreReason> {
        // the transmit_timestamp field was not changed from the bogus value we put into it
        let transmit_unchanged = Some(message.transmit_timestamp) == self.next_expected_origin;

        // the origin_timestamp changed; the server is not allowed to modify this field
        let origin_changed = Some(message.origin_timestamp) != self.next_expected_origin;

        if message.mode != NtpAssociationMode::Server {
            // we currently only support a client <-> server association
            Err(IgnoreReason::InvalidMode)
        } else if transmit_unchanged || origin_changed {
            Err(IgnoreReason::InvalidPacketTime)
        } else if message.is_kiss_rate() {
            self.remote_min_poll_interval =
                Ord::max(self.remote_min_poll_interval.inc(), self.last_poll_interval);
            Err(IgnoreReason::KissIgnore)
        } else if message.is_kiss_rstr() || message.is_kiss_deny() {
            Err(IgnoreReason::KissDemobilize)
        } else if message.is_kiss() {
            // Ignore unrecognized control messages
            Err(IgnoreReason::KissIgnore)
        } else {
            // For reachability, mark that we have had a response
            self.reach.received_packet();

            // Received answer, so no need for backoff
            self.next_poll_interval = self.last_poll_interval;

            let filter_input = FilterTuple::from_packet_default(
                &message,
                system.precision,
                local_clock_time,
                frequency_tolerance,
                recv_time,
            );

            self.message_for_system(
                filter_input,
                system.leap_indicator,
                system.precision,
                frequency_tolerance,
            )
        }
    }

    /// Data from a peer that is needed for the (global) clock filter and combine process
    fn message_for_system(
        &mut self,
        new_tuple: FilterTuple,
        system_leap_indicator: NtpLeapIndicator,
        system_precision: NtpDuration,
        frequency_tolerance: FrequencyTolerance,
    ) -> Result<PeerSnapshot, IgnoreReason> {
        let updated = self.last_measurements.step(
            new_tuple,
            self.time,
            system_leap_indicator,
            system_precision,
            frequency_tolerance,
        );

        match updated {
            None => Err(IgnoreReason::TooOld),
            Some((statistics, smallest_delay_time)) => {
                self.statistics = statistics;
                self.time = smallest_delay_time;

                let snapshot = PeerSnapshot {
                    time: self.time,
                    root_distance_without_time: self.root_distance_without_time(),
                    stratum: self.last_packet.stratum,
                    statistics: self.statistics,
                };

                Ok(snapshot)
            }
        }
    }

    /// The root synchronization distance is the maximum error due to
    /// all causes of the local clock relative to the primary server.
    /// It is defined as half the total delay plus total dispersion
    /// plus peer jitter.
    fn root_distance(
        &self,
        local_clock_time: NtpInstant,
        frequency_tolerance: FrequencyTolerance,
    ) -> NtpDuration {
        self.root_distance_without_time() + ((local_clock_time - self.time) * frequency_tolerance)
    }

    /// Root distance without the `(local_clock_time - self.time) * PHI` term
    fn root_distance_without_time(&self) -> NtpDuration {
        NtpDuration::MIN_DISPERSION.max(self.last_packet.root_delay + self.statistics.delay) / 2i64
            + self.last_packet.root_dispersion
            + self.statistics.dispersion
            + NtpDuration::from_seconds(self.statistics.jitter)
    }

    /// Test if association p is acceptable for synchronization
    ///
    /// Known as `accept` and `fit` in the specification.
    pub fn accept_synchronization(
        &self,
        local_clock_time: NtpInstant,
        frequency_tolerance: FrequencyTolerance,
        distance_threshold: NtpDuration,
        system_poll: NtpDuration,
    ) -> Result<(), AcceptSynchronizationError> {
        use AcceptSynchronizationError::*;

        // A stratum error occurs if
        //     1: the server has never been synchronized,
        //     2: the server stratum is invalid
        if !self.last_packet.leap.is_synchronized() || self.last_packet.stratum >= MAX_STRATUM {
            return Err(Stratum);
        }

        //  A distance error occurs if the root distance exceeds the
        //  distance threshold plus an increment equal to one poll interval.
        let distance = self.root_distance(local_clock_time, frequency_tolerance);
        if distance > distance_threshold + (system_poll * frequency_tolerance) {
            return Err(Distance);
        }

        // Detect whether the remote uses us as their main time reference.
        // if so, we shouldn't sync to them as that would create a loop.
        // Note, this can only ever be an issue if the peer is not using
        // hardware as its source, so ignore reference_id if stratum is 1.
        if self.last_packet.stratum != 1 && self.last_packet.reference_id == self.our_id {
            return Err(Loop);
        }

        // An unreachable error occurs if the server is unreachable.
        if !self.reach.is_reachable() {
            return Err(ServerUnreachable);
        }

        Ok(())
    }

    #[cfg(any(test, feature = "fuzz"))]
    pub(crate) fn test_peer() -> Self {
        Peer {
            last_poll_interval: PollInterval::default(),
            next_poll_interval: PollInterval::default(),
            remote_min_poll_interval: PollInterval::default(),

            next_expected_origin: None,

            statistics: Default::default(),
            last_measurements: Default::default(),
            last_packet: Default::default(),
            time: Default::default(),
            peer_id: ReferenceId::from_int(0),
            our_id: ReferenceId::from_int(0),
            reach: Reach::default(),
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_root_duration_sanity() {
        // Ensure root distance at least increases as it is supposed to
        // when changing the main measurement parameters

        let duration_1s = NtpDuration::from_fixed_int(1_0000_0000);
        let duration_2s = NtpDuration::from_fixed_int(2_0000_0000);

        let timestamp_1s = NtpInstant::from_fixed_int(1_0000_0000);
        let timestamp_2s = NtpInstant::from_fixed_int(2_0000_0000);

        let ft = FrequencyTolerance::ppm(15);

        let mut packet = NtpHeader::new();
        packet.root_delay = duration_1s;
        packet.root_dispersion = duration_1s;
        let reference = Peer {
            statistics: PeerStatistics {
                delay: duration_1s,
                dispersion: duration_1s,
                ..Default::default()
            },
            last_packet: packet,
            time: timestamp_1s,
            ..Peer::test_peer()
        };

        assert!(
            reference.root_distance(timestamp_1s, ft) < reference.root_distance(timestamp_2s, ft)
        );

        let sample = Peer {
            statistics: PeerStatistics {
                delay: duration_2s,
                dispersion: duration_1s,
                ..Default::default()
            },
            last_packet: packet,
            time: timestamp_1s,
            ..Peer::test_peer()
        };
        assert!(reference.root_distance(timestamp_1s, ft) < sample.root_distance(timestamp_1s, ft));

        let sample = Peer {
            statistics: PeerStatistics {
                delay: duration_1s,
                dispersion: duration_2s,
                ..Default::default()
            },
            last_packet: packet,
            time: timestamp_1s,
            ..Peer::test_peer()
        };
        assert!(reference.root_distance(timestamp_1s, ft) < sample.root_distance(timestamp_1s, ft));

        let sample = Peer {
            statistics: PeerStatistics {
                delay: duration_1s,
                dispersion: duration_1s,
                ..Default::default()
            },
            last_packet: packet,
            time: NtpInstant::ZERO,
            ..Peer::test_peer()
        };
        assert!(reference.root_distance(timestamp_1s, ft) < sample.root_distance(timestamp_1s, ft));

        packet.root_delay = duration_2s;
        let sample = Peer {
            statistics: PeerStatistics {
                delay: duration_1s,
                dispersion: duration_1s,
                ..Default::default()
            },
            last_packet: packet,
            time: timestamp_1s,
            ..Peer::test_peer()
        };
        packet.root_delay = duration_1s;
        assert!(reference.root_distance(timestamp_1s, ft) < sample.root_distance(timestamp_1s, ft));

        packet.root_dispersion = duration_2s;
        let sample = Peer {
            statistics: PeerStatistics {
                delay: duration_1s,
                dispersion: duration_1s,
                ..Default::default()
            },
            last_packet: packet,
            time: timestamp_1s,
            ..Peer::test_peer()
        };
        packet.root_dispersion = duration_1s;
        assert!(reference.root_distance(timestamp_1s, ft) < sample.root_distance(timestamp_1s, ft));

        let sample = Peer {
            statistics: PeerStatistics {
                delay: duration_1s,
                dispersion: duration_1s,
                ..Default::default()
            },
            last_packet: packet,
            time: timestamp_1s,
            ..Peer::test_peer()
        };

        assert_eq!(
            reference.root_distance(timestamp_1s, ft),
            sample.root_distance(timestamp_1s, ft)
        );
    }

    #[test]
    fn reachability() {
        let mut reach = Reach::default();

        // the default reach register value is 0, and hence not reachable
        assert!(!reach.is_reachable());

        // when we receive a packet, we set the right-most bit;
        // we just received a packet from the peer, so it is reachable
        reach.received_packet();
        assert!(reach.is_reachable());

        // on every poll, the register is shifted to the left, and there are
        // 8 bits. So we can poll 7 times and the peer is still considered reachable
        for _ in 0..7 {
            reach.poll();
        }

        assert!(reach.is_reachable());

        // but one more poll and all 1 bits have been shifted out;
        // the peer is no longer reachable
        reach.poll();
        assert!(!reach.is_reachable());

        // until we receive a packet from it again
        reach.received_packet();
        assert!(reach.is_reachable());
    }

    #[test]
    fn test_accept_synchronization() {
        use AcceptSynchronizationError::*;

        let local_clock_time = NtpInstant::ZERO;
        let ft = FrequencyTolerance::ppm(15);
        let dt = NtpDuration::ONE;
        let system_poll = NtpDuration::ZERO;

        let mut peer = Peer::test_peer();

        // by default, the packet id and the peer's id are the same, indicating a loop
        assert_eq!(
            peer.accept_synchronization(local_clock_time, ft, dt, system_poll),
            Err(Loop)
        );

        peer.our_id = ReferenceId::from_int(42);

        assert_eq!(
            peer.accept_synchronization(local_clock_time, ft, dt, system_poll),
            Err(ServerUnreachable)
        );

        peer.reach.received_packet();

        assert_eq!(
            peer.accept_synchronization(local_clock_time, ft, dt, system_poll),
            Ok(())
        );

        peer.last_packet.leap = NtpLeapIndicator::Unknown;
        assert_eq!(
            peer.accept_synchronization(local_clock_time, ft, dt, system_poll),
            Err(Stratum)
        );

        peer.last_packet.leap = NtpLeapIndicator::NoWarning;
        peer.last_packet.stratum = 42;
        assert_eq!(
            peer.accept_synchronization(local_clock_time, ft, dt, system_poll),
            Err(Stratum)
        );

        peer.last_packet.stratum = 0;

        peer.last_packet.root_dispersion = dt * 2;
        assert_eq!(
            peer.accept_synchronization(local_clock_time, ft, dt, system_poll),
            Err(Distance)
        );
    }
}