//! Battery / AC state from `GetSystemPowerStatus` — free and instant.

use super::ffi;
use super::zeroed;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BatteryState {
    Charging,
    Discharging,
    Idle,
    NoBattery,
    Unknown,
}

#[derive(Clone, Copy, Debug)]
pub struct Power {
    pub ac_online: bool,
    /// `None` when the firmware reports 255 ("unknown").
    pub battery_percent: Option<u8>,
    pub state: BatteryState,
}

impl Power {
    pub fn available(&self) -> bool {
        self.state != BatteryState::NoBattery
    }
}

pub fn read() -> Option<Power> {
    let mut status: ffi::SYSTEM_POWER_STATUS = zeroed();
    if super::failed(unsafe { ffi::GetSystemPowerStatus(&mut status) }) {
        return None;
    }

    let state = if status.BatteryFlag == 128 {
        BatteryState::NoBattery
    } else if status.BatteryFlag == 255 {
        BatteryState::Unknown
    } else if status.BatteryFlag & 8 != 0 {
        BatteryState::Charging
    } else if status.ACLineStatus == 0 {
        BatteryState::Discharging
    } else if status.BatteryLifePercent != 255 {
        BatteryState::Idle
    } else {
        BatteryState::Unknown
    };

    Some(Power {
        ac_online: status.ACLineStatus == 1,
        battery_percent: (status.BatteryLifePercent <= 100).then_some(status.BatteryLifePercent),
        state,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_power_state() {
        let power = read().expect("GetSystemPowerStatus");
        if let Some(percent) = power.battery_percent {
            assert!(percent <= 100);
        }
    }
}
