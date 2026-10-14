//! M2.7-2: every GPU adapter of the machine, with a stable id. Listing
//! creates no device; it is what `signal devices --compute` and the app's
//! interface setup show, and what a [`DevicePool`](crate::DevicePool) opens.

#[cfg(feature = "noop")]
use crate::GpuError;

/// The compute limits of an adapter that decide whether a channel can run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AdapterLimits {
    /// Workgroups per dimension of one dispatch.
    pub max_compute_workgroups_per_dimension: u32,
    /// Invocations in one workgroup.
    pub max_compute_invocations_per_workgroup: u32,
    /// A workgroup's size in x, y and z.
    pub max_compute_workgroup_size: (u32, u32, u32),
    /// Workgroup storage, in bytes.
    pub max_compute_workgroup_storage_size: u32,
    /// One storage buffer binding, in bytes.
    pub max_storage_buffer_binding_size: u64,
    /// One buffer, in bytes.
    pub max_buffer_size: u64,
}

/// One adapter, as the inventory lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdapterInfo {
    /// Its stable id: `<backend>-<name>` in lowercase with every run of
    /// other characters as `-`, then `-<PCI bus id>` where the backend
    /// reports one; adapters still alike are told apart by `-2`, `-3`… in
    /// the order the backend lists them. Never all digits (a number is an
    /// index). An assumption: wgpu exposes no Metal registry id, so the id
    /// holds while the machine's adapters and their names stay the same.
    pub id: String,
    /// Its place in the inventory, which is sorted by id.
    pub index: usize,
    /// Its name.
    pub name: String,
    /// The backend (`Metal`; `Noop` for the test device).
    pub backend: String,
    /// `integrated`, `discrete`, `virtual`, `cpu` or `other`.
    pub device_type: &'static str,
    /// Its PCI vendor id (0 where the backend has none).
    pub vendor: u32,
    /// Its PCI device id (0 where the backend has none).
    pub device_id: u32,
    /// The driver's name.
    pub driver: String,
    /// The driver's version or other information.
    pub driver_info: String,
    /// Its compute limits.
    pub limits: AdapterLimits,
}

/// Every adapter, sorted by id, or why there are none.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Inventory {
    /// The adapters.
    pub adapters: Vec<AdapterInfo>,
    /// Why the list is empty, when it is.
    pub error: Option<String>,
}

/// Every Metal adapter of this machine. Control side: it asks the backend
/// (no device is created).
pub fn inventory() -> Inventory {
    let adapters: Vec<AdapterInfo> = adapters(metal())
        .into_iter()
        .map(|(info, _)| info)
        .collect();
    let error = adapters
        .is_empty()
        .then(|| "no Metal adapter (no usable GPU on this machine)".to_owned());
    Inventory { adapters, error }
}

impl Inventory {
    /// The GPU part of the compute inventory for people, one block per
    /// adapter (`signal devices --compute` and the app's interface setup).
    pub fn lines(&self) -> Vec<String> {
        let mut lines = vec!["GPU:".to_owned()];
        if let Some(error) = &self.error {
            lines.push(format!("  none: {error}"));
        }
        for a in &self.adapters {
            let l = &a.limits;
            let (x, y, z) = l.max_compute_workgroup_size;
            lines.push(format!(
                "  {} {}: {} ({}, {})",
                a.index, a.id, a.name, a.backend, a.device_type
            ));
            lines.push(format!(
                "    vendor {:#06x}, device {:#06x}, driver {} {}",
                a.vendor, a.device_id, a.driver, a.driver_info
            ));
            lines.push(format!(
                "    limits: {} workgroups per dimension, {} invocations per workgroup ({x} x {y} x {z}), {} B workgroup storage, {} B storage binding, {} B buffer",
                l.max_compute_workgroups_per_dimension,
                l.max_compute_invocations_per_workgroup,
                l.max_compute_workgroup_storage_size,
                l.max_storage_buffer_binding_size,
                l.max_buffer_size
            ));
        }
        lines
    }
}

pub(crate) fn metal() -> wgpu::Instance {
    wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::METAL,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    })
}

/// Every adapter of `instance` with its info, sorted by id.
pub(crate) fn adapters(instance: wgpu::Instance) -> Vec<(AdapterInfo, wgpu::Adapter)> {
    let listed = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()));
    let mut bases: Vec<String> = Vec::new();
    let mut found: Vec<(AdapterInfo, wgpu::Adapter)> = Vec::new();
    for adapter in listed {
        let raw = adapter.get_info();
        let mut base = format!(
            "{}-{}",
            slug(&format!("{:?}", raw.backend)),
            slug(&raw.name)
        );
        if !raw.device_pci_bus_id.is_empty() {
            base = format!("{base}-{}", slug(&raw.device_pci_bus_id));
        }
        let alike = bases.iter().filter(|b| **b == base).count();
        let id = match alike {
            0 => base.clone(),
            n => format!("{base}-{}", n + 1),
        };
        bases.push(base);
        found.push((describe(id, &raw, &adapter.limits()), adapter));
    }
    found.sort_by(|a, b| a.0.id.cmp(&b.0.id));
    for (index, (info, _)) in found.iter_mut().enumerate() {
        info.index = index;
    }
    found
}

/// The inventory's record of an adapter with `id`.
pub(crate) fn describe(id: String, raw: &wgpu::AdapterInfo, limits: &wgpu::Limits) -> AdapterInfo {
    AdapterInfo {
        id,
        index: 0,
        name: raw.name.clone(),
        backend: format!("{:?}", raw.backend),
        device_type: match raw.device_type {
            wgpu::DeviceType::IntegratedGpu => "integrated",
            wgpu::DeviceType::DiscreteGpu => "discrete",
            wgpu::DeviceType::VirtualGpu => "virtual",
            wgpu::DeviceType::Cpu => "cpu",
            wgpu::DeviceType::Other => "other",
        },
        vendor: raw.vendor,
        device_id: raw.device,
        driver: raw.driver.clone(),
        driver_info: raw.driver_info.clone(),
        limits: AdapterLimits {
            max_compute_workgroups_per_dimension: limits.max_compute_workgroups_per_dimension,
            max_compute_invocations_per_workgroup: limits.max_compute_invocations_per_workgroup,
            max_compute_workgroup_size: (
                limits.max_compute_workgroup_size_x,
                limits.max_compute_workgroup_size_y,
                limits.max_compute_workgroup_size_z,
            ),
            max_compute_workgroup_storage_size: limits.max_compute_workgroup_storage_size,
            max_storage_buffer_binding_size: limits.max_storage_buffer_binding_size,
            max_buffer_size: limits.max_buffer_size,
        },
    }
}

/// wgpu's noop adapter, from an instance of its own (tests only).
#[cfg(feature = "noop")]
pub(crate) fn noop_adapter() -> Result<wgpu::Adapter, GpuError> {
    let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
    descriptor.backends = wgpu::Backends::NOOP;
    descriptor.backend_options.noop.enable = true;
    let instance = wgpu::Instance::new(descriptor);
    pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
        .map_err(|e| GpuError::NoDevice(e.to_string()))
}

/// Lowercase ASCII letters and digits; every other run is one `-`.
fn slug(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches('-');
    if out.is_empty() {
        "adapter".to_owned()
    } else {
        out.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_slugs_and_never_numbers() {
        assert_eq!(slug("Apple M5 Max"), "apple-m5-max");
        assert_eq!(slug("  ** "), "adapter");
        assert_eq!(slug("Metal"), "metal");
    }

    /// The noop adapter is listed with an id from its backend and name.
    #[test]
    fn the_noop_adapter_has_a_stable_id() {
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
        descriptor.backends = wgpu::Backends::NOOP;
        descriptor.backend_options.noop.enable = true;
        let listed = adapters(wgpu::Instance::new(descriptor));
        let ids: Vec<&str> = listed.iter().map(|(i, _)| i.id.as_str()).collect();
        eprintln!("noop adapters: {ids:?}");
        assert!(ids.iter().all(|id| id.starts_with("noop-")), "{ids:?}");
        assert!(ids.iter().all(|id| id.parse::<usize>().is_err()));
    }
}
