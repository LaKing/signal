//! Audit P1 (2026-10-15, `docs/semantics.md` §16): what a project names can
//! never make the engine allocate without bound. A vector is at most
//! `MAX_VECTOR_WIDTH` elements, and a plan has at most `MAX_PLAN_INSTANCES`
//! instances and `MAX_PLAN_ROWS` value rows; each is refused with a named
//! reason when the plan is built, before any engine sees it.

mod common;

use common::{addr, build, composite, errors, inst, registry};
use signal_core::{MAX_VECTOR_WIDTH, ModuleType, PortDescriptor, PortShape};
use signal_graph::{GraphError, MAX_PLAN_INSTANCES, MAX_PLAN_ROWS, PlanBudget, build_plan};

/// A converter of `n` elements, named as a project names one.
fn converter(n: u32) -> String {
    format!("core.hold.d2.d1.vector{n}.generic")
}

/// The audit's probe: a converter of 2³² − 1 elements, whose one 256-frame
/// buffer alone would be 8 TiB, is refused for its width, not accepted (and
/// not merely unknown).
#[test]
fn the_audit_probe_is_refused_for_its_width() {
    let module = converter(u32::MAX);
    let root = composite("audit", vec![inst("huge", &module)], vec![]);
    let found = errors(&root, &[]);
    assert_eq!(
        found,
        [GraphError::VectorTooWide {
            instance: addr("/huge"),
            module: module.clone(),
            width: u32::MAX,
        }]
    );
    assert_eq!(
        found[0].to_string(),
        format!("/huge: `{module}` has a vector of 4294967295 elements; a vector has at most 4096")
    );
}

/// The widest vector plans; one more element does not.
#[test]
fn the_widest_vector_plans_and_one_more_is_refused() {
    let widest = composite(
        "project",
        vec![inst("h", &converter(MAX_VECTOR_WIDTH))],
        vec![],
    );
    let plan = build(&widest, &[]).unwrap();
    let shape = plan.instances[0].descriptor.ports[0].shape;
    assert_eq!(shape, PortShape::Vector(MAX_VECTOR_WIDTH));

    let wider = composite(
        "project",
        vec![inst("h", &converter(MAX_VECTOR_WIDTH + 1))],
        vec![],
    );
    assert!(matches!(
        errors(&wider, &[])[..],
        [GraphError::VectorTooWide { width, .. }] if width == MAX_VECTOR_WIDTH + 1
    ));
}

/// A registered type is held to the same width as a converter.
#[test]
fn a_registered_type_wider_than_the_limit_is_refused() {
    let mut registry = registry();
    let vsrc = *registry.get("test.vsrc").unwrap();
    let mut descriptor = vsrc.descriptor;
    descriptor.type_name = "test.too_wide";
    descriptor.ports = Box::leak(Box::new([PortDescriptor {
        shape: PortShape::Vector(MAX_VECTOR_WIDTH + 1),
        ..descriptor.ports[0]
    }]));
    registry
        .register(ModuleType {
            descriptor,
            create: vsrc.create,
        })
        .unwrap();
    let root = composite("project", vec![inst("w", "test.too_wide")], vec![]);
    assert_eq!(
        build_plan(&root, &[], &registry, 1).unwrap_err(),
        [GraphError::VectorTooWide {
            instance: addr("/w"),
            module: "test.too_wide".to_owned(),
            width: MAX_VECTOR_WIDTH + 1,
        }]
    );
}

/// Value rows are budgeted for the whole plan: each widest converter has
/// two ports of 4096 rows, so 64 of them are exactly the budget and 65 are
/// over it, each one allowed on its own.
#[test]
fn a_plan_over_the_row_budget_is_refused() {
    let converters = |n: usize| {
        let instances = (0..n)
            .map(|i| inst(&format!("h{i}"), &converter(MAX_VECTOR_WIDTH)))
            .collect();
        composite("project", instances, vec![])
    };
    let per = 2 * MAX_VECTOR_WIDTH as usize;
    let fits = MAX_PLAN_ROWS / per;
    assert_eq!(fits * per, MAX_PLAN_ROWS);
    assert_eq!(build(&converters(fits), &[]).unwrap().instances.len(), fits);
    assert_eq!(
        errors(&converters(fits + 1), &[]),
        [GraphError::PlanTooLarge {
            budget: PlanBudget::Rows,
            count: MAX_PLAN_ROWS + per,
        }]
    );
}

/// Composites nested eight wide and seven deep would flatten to 2²¹
/// oscillators from a few lines of text. Flattening stops at the instance
/// budget, having made one instance more than it allows.
#[test]
fn composites_expanding_past_the_instance_budget_stop_there() {
    let wide = |name: &str, module: &str| {
        let instances = (0..8).map(|i| inst(&format!("i{i}"), module)).collect();
        composite(name, instances, vec![])
    };
    let mut library = vec![wide("c0", "test.osc")];
    for level in 1..7 {
        library.push(wide(&format!("c{level}"), &format!("c{}", level - 1)));
    }
    let root = wide("project", "c6");
    assert_eq!(
        errors(&root, &library),
        [GraphError::PlanTooLarge {
            budget: PlanBudget::Instances,
            count: MAX_PLAN_INSTANCES + 1,
        }]
    );
}
