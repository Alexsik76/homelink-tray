//! A problem with one field of a form, in a shape the UI can show under the field.
//!
//! A [`Problem`] has no wording: it names the field, a [`Code`] and the parameters of the
//! message. [`crate::i18n::Translator::problem`] renders it from the locale key
//! `problem.<code>`. Field names are stable strings documented by the form that reports them
//! (`wizard::field`, `tunnel_form::field`, `settings`).

use crate::rules::Fault;

/// What is wrong; the suffix of the locale key `problem.<code>` (see [`Code::as_str`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Code {
    /// The field is empty. No parameters.
    Required,
    /// Another entry already has this name. Parameter: `name`.
    NameTaken,
    /// Not an IP address or a host name. Parameter: `value`.
    HostInvalid,
    /// No port was given. No parameters.
    PortsEmpty,
    /// Not a port number from 1 to 65535. Parameter: `value`.
    PortInvalid,
    /// The same port twice. Parameter: `port`.
    PortDuplicate,
    /// Not a MAC address. Parameter: `value`.
    MacInvalid,
    /// Not an IPv4 address. Parameter: `value`.
    Ipv4Invalid,
    /// Not a valid interface name. Parameter: `value`.
    InterfaceInvalid,
    /// The relay preset needs an interface. Parameter: `preset`.
    InterfaceRequired,
    /// The relay preset does not use an interface. Parameter: `preset`.
    InterfaceNotUsed,
    /// The relay preset does not take a destination address. Parameter: `preset`.
    AddressNotUsed,
    /// The relay preset needs a yes/no answer for sudo. Parameter: `preset`.
    SudoRequired,
    /// The SSH user has blanks, `@` or starts with `-`. Parameter: `value`.
    UserInvalid,
    /// The key file path is not absolute. Parameter: `value`.
    KeyNotAbsolute,
    /// The WireGuard file is not `.conf` or `.conf.dpapi`. Parameter: `value`.
    ConfExtension,
    /// The WireGuard tunnel name derived from the file name is not allowed. Parameter: `name`.
    ConfName,
    /// Not a whole number. Parameter: `value`.
    NumberInvalid,
    /// A number of seconds outside 1..max. Parameter: `max`.
    SecondsRange,
    /// A number of milliseconds outside 1..max. Parameter: `max`.
    MillisRange,
    /// A quote is not closed. No parameters.
    CommandQuote,
    /// An unknown `{placeholder}`. Parameters: `name`, `allowed`.
    CommandPlaceholder,
    /// Unbalanced braces. No parameters.
    CommandBraces,
    /// Over SSH, sleep and shutdown are both available: both must be allowed. No parameters.
    PowerSshNeedsBoth,
    /// No relay is chosen, or the chosen one does not exist. No parameters.
    RelayUnknown,
    /// The chosen tunnel does not exist. No parameters.
    TunnelUnknown,
    /// A tunnel cannot be deleted while PCs use it. Parameters: `tunnel`, `pcs`.
    TunnelInUse,
}

impl Code {
    /// Every code, for tests.
    pub const ALL: [Code; 27] = [
        Self::Required,
        Self::NameTaken,
        Self::HostInvalid,
        Self::PortsEmpty,
        Self::PortInvalid,
        Self::PortDuplicate,
        Self::MacInvalid,
        Self::Ipv4Invalid,
        Self::InterfaceInvalid,
        Self::InterfaceRequired,
        Self::InterfaceNotUsed,
        Self::AddressNotUsed,
        Self::SudoRequired,
        Self::UserInvalid,
        Self::KeyNotAbsolute,
        Self::ConfExtension,
        Self::ConfName,
        Self::NumberInvalid,
        Self::SecondsRange,
        Self::MillisRange,
        Self::CommandQuote,
        Self::CommandPlaceholder,
        Self::CommandBraces,
        Self::PowerSshNeedsBoth,
        Self::RelayUnknown,
        Self::TunnelUnknown,
        Self::TunnelInUse,
    ];

    /// The suffix of the locale key: `problem.<as_str>`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Required => "required",
            Self::NameTaken => "name_taken",
            Self::HostInvalid => "host_invalid",
            Self::PortsEmpty => "ports_empty",
            Self::PortInvalid => "port_invalid",
            Self::PortDuplicate => "port_duplicate",
            Self::MacInvalid => "mac_invalid",
            Self::Ipv4Invalid => "ipv4_invalid",
            Self::InterfaceInvalid => "interface_invalid",
            Self::InterfaceRequired => "interface_required",
            Self::InterfaceNotUsed => "interface_not_used",
            Self::AddressNotUsed => "address_not_used",
            Self::SudoRequired => "sudo_required",
            Self::UserInvalid => "user_invalid",
            Self::KeyNotAbsolute => "key_not_absolute",
            Self::ConfExtension => "conf_extension",
            Self::ConfName => "conf_name",
            Self::NumberInvalid => "number_invalid",
            Self::SecondsRange => "seconds_range",
            Self::MillisRange => "millis_range",
            Self::CommandQuote => "command_quote",
            Self::CommandPlaceholder => "command_placeholder",
            Self::CommandBraces => "command_braces",
            Self::PowerSshNeedsBoth => "power_ssh_needs_both",
            Self::RelayUnknown => "relay_unknown",
            Self::TunnelUnknown => "tunnel_unknown",
            Self::TunnelInUse => "tunnel_in_use",
        }
    }

    /// The parameters the message of this code may use.
    pub fn params(self) -> &'static [&'static str] {
        match self {
            Self::Required
            | Self::PortsEmpty
            | Self::CommandQuote
            | Self::CommandBraces
            | Self::PowerSshNeedsBoth
            | Self::RelayUnknown
            | Self::TunnelUnknown => &[],
            Self::NameTaken | Self::ConfName => &["name"],
            Self::HostInvalid
            | Self::PortInvalid
            | Self::MacInvalid
            | Self::Ipv4Invalid
            | Self::InterfaceInvalid
            | Self::UserInvalid
            | Self::KeyNotAbsolute
            | Self::ConfExtension
            | Self::NumberInvalid => &["value"],
            Self::PortDuplicate => &["port"],
            Self::InterfaceRequired
            | Self::InterfaceNotUsed
            | Self::AddressNotUsed
            | Self::SudoRequired => &["preset"],
            Self::SecondsRange | Self::MillisRange => &["max"],
            Self::CommandPlaceholder => &["name", "allowed"],
            Self::TunnelInUse => &["tunnel", "pcs"],
        }
    }
}

/// One problem with one field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// The field, a stable name such as `host` or `relay.port` (see the form's `field` module).
    pub field: &'static str,
    /// What is wrong.
    pub code: Code,
    /// Values for the `{placeholders}` of the message, as `(name, value)`.
    pub params: Vec<(&'static str, String)>,
}

impl Problem {
    /// A problem without parameters.
    pub fn new(field: &'static str, code: Code) -> Self {
        Self {
            field,
            code,
            params: Vec::new(),
        }
    }

    /// Adds a parameter.
    #[must_use]
    pub fn with(mut self, name: &'static str, value: impl Into<String>) -> Self {
        self.params.push((name, value.into()));
        self
    }

    /// The first problem reported for `field`.
    pub fn first_for<'a>(problems: &'a [Problem], field: &str) -> Option<&'a Problem> {
        problems.iter().find(|p| p.field == field)
    }
}

/// The problem for an empty or otherwise wrong common value; used by the forms.
pub(crate) fn from_fault(field: &'static str, fault: Fault, wrong: Code, value: &str) -> Problem {
    match fault {
        Fault::Empty => Problem::new(field, Code::Required),
        _ => Problem::new(field, wrong).with("value", value),
    }
}
