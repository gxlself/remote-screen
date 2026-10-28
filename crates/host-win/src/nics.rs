#![cfg(windows)]

//! Thin Windows adapter enumeration layer.

use std::mem::size_of;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use host_core::platform::Nic;
use windows::core::PWSTR;
use windows::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, ERROR_SUCCESS};
use windows::Win32::NetworkManagement::IpHelper::{
    GetAdaptersAddresses, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST,
    IP_ADAPTER_ADDRESSES_LH, IP_ADAPTER_UNICAST_ADDRESS_LH,
};
use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
use windows::Win32::Networking::WinSock::{
    AF_INET, AF_INET6, AF_UNSPEC, SOCKADDR_IN, SOCKADDR_IN6,
};

use host_core::nic_model::{to_nics, AdapterRecord};

const INITIAL_BUFFER_SIZE: u32 = 15 * 1024;
const MAX_QUERY_ATTEMPTS: usize = 3;

/// Returns the active Windows network adapters with their unicast addresses.
pub fn nics() -> Vec<Nic> {
    let Some(records) = query_adapters() else {
        return Vec::new();
    };
    to_nics(&records)
}

fn query_adapters() -> Option<Vec<AdapterRecord>> {
    let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
    let mut buffer_size = INITIAL_BUFFER_SIZE;

    for attempt in 0..MAX_QUERY_ATTEMPTS {
        let bytes = usize::try_from(buffer_size).ok()?;
        let words = bytes.checked_add(size_of::<usize>() - 1)? / size_of::<usize>();
        let mut buffer = vec![0_usize; words.max(1)];
        let mut returned_size = buffer_size;
        let status = unsafe {
            // SAFETY: `buffer` is writable, suitably aligned storage whose
            // size is described by `returned_size`; all optional pointers are
            // null as required by GetAdaptersAddresses.
            GetAdaptersAddresses(
                AF_UNSPEC.0 as u32,
                flags,
                None,
                Some(buffer.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>()),
                &mut returned_size,
            )
        };

        if status == ERROR_SUCCESS.0 {
            return Some(adapter_records(
                buffer.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>(),
            ));
        }

        if status == ERROR_BUFFER_OVERFLOW.0 && attempt + 1 < MAX_QUERY_ATTEMPTS {
            buffer_size = returned_size.max(INITIAL_BUFFER_SIZE);
            continue;
        }

        tracing::warn!(status, "GetAdaptersAddresses failed");
        return None;
    }

    None
}

fn adapter_records(mut current: *mut IP_ADAPTER_ADDRESSES_LH) -> Vec<AdapterRecord> {
    let mut records = Vec::new();
    while !current.is_null() {
        let adapter = unsafe {
            // SAFETY: the pointer is part of the live buffer returned by
            // GetAdaptersAddresses and remains valid until query_adapters
            // drops that buffer after this function returns.
            &*current
        };
        let mac_len = usize::try_from(adapter.PhysicalAddressLength)
            .unwrap_or(usize::MAX)
            .min(adapter.PhysicalAddress.len());
        records.push(AdapterRecord {
            friendly_name: wide_string(adapter.FriendlyName),
            description: wide_string(adapter.Description),
            if_type: adapter.IfType,
            oper_up: adapter.OperStatus == IfOperStatusUp,
            mac: adapter.PhysicalAddress[..mac_len].to_vec(),
            unicast: unicast_addresses(adapter.FirstUnicastAddress),
        });
        current = adapter.Next;
    }
    records
}

fn unicast_addresses(mut current: *mut IP_ADAPTER_UNICAST_ADDRESS_LH) -> Vec<(IpAddr, u8)> {
    let mut addresses = Vec::new();
    while !current.is_null() {
        let unicast = unsafe {
            // SAFETY: the pointer is a node in the live adapter buffer.
            &*current
        };
        if let Some(address) =
            ip_from_sockaddr(unicast.Address.lpSockaddr, unicast.Address.iSockaddrLength)
        {
            addresses.push((address, unicast.OnLinkPrefixLength));
        }
        current = unicast.Next;
    }
    addresses
}

fn wide_string(value: PWSTR) -> String {
    if value.is_null() {
        return String::new();
    }
    let wide = unsafe {
        // SAFETY: FriendlyName and Description point to NUL-terminated UTF-16
        // strings owned by the live GetAdaptersAddresses buffer.
        value.as_wide()
    };
    String::from_utf16_lossy(wide)
}

fn ip_from_sockaddr(
    address: *const windows::Win32::Networking::WinSock::SOCKADDR,
    length: i32,
) -> Option<IpAddr> {
    if address.is_null() {
        return None;
    }
    let length = usize::try_from(length).ok()?;
    let family = unsafe {
        // SAFETY: the pointer is non-null and points to at least a SOCKADDR;
        // GetAdaptersAddresses supplies the socket address length.
        (*address).sa_family
    };
    if family == AF_INET {
        if length < size_of::<SOCKADDR_IN>() {
            return None;
        }
        let octets = unsafe {
            // SAFETY: the family and length establish that this is a complete
            // SOCKADDR_IN supplied by GetAdaptersAddresses.
            (*address.cast::<SOCKADDR_IN>())
                .sin_addr
                .S_un
                .S_addr
                .to_ne_bytes()
        };
        return Some(IpAddr::V4(Ipv4Addr::from(octets)));
    }
    if family == AF_INET6 {
        if length < size_of::<SOCKADDR_IN6>() {
            return None;
        }
        let octets = unsafe {
            // SAFETY: the family and length establish that this is a complete
            // SOCKADDR_IN6 supplied by GetAdaptersAddresses.
            (*address.cast::<SOCKADDR_IN6>()).sin6_addr.u.Byte
        };
        return Some(IpAddr::V6(Ipv6Addr::from(octets)));
    }
    None
}
