use super::SignalName;
use super::SignalValue;

#[derive(Clone, Debug, PartialEq)]
pub struct SignalShow {
    pub signal_name: SignalName,
    pub signal_show: SignalValue,
}