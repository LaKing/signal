//! The module contract compiles as intended: both primitive traits are object
//! safe, and the registry stores module types by unique name.

use signal_core::{
    DirectPrimitive, Direction, Event, EventPayload, EventSink, Feedthrough, Inputs,
    InstanceAddress, ModuleDescriptor, ModuleType, Outputs, PortDescriptor, PortKind, PortShape,
    PrepareContext, Primitive, RateDivisor, Registry, RegistryError, RngStream,
    StateOutputPrimitive,
};

const fn port(name: &'static str, direction: Direction, shape: PortShape) -> PortDescriptor {
    PortDescriptor {
        name,
        direction,
        shape,
        rate: RateDivisor::ONE,
        kind: PortKind::Generic,
        unit: None,
        range: None,
    }
}

const PORTS: &[PortDescriptor] = &[
    port("in", Direction::In, PortShape::Signal),
    port("out", Direction::Out, PortShape::Signal),
    port("events", Direction::Out, PortShape::Events),
];

const fn descriptor(type_name: &'static str, feedthrough: Feedthrough) -> ModuleDescriptor {
    ModuleDescriptor {
        type_name,
        doc: "test primitive",
        ports: PORTS,
        parameters: &[],
        feedthrough,
        sink: None,
        latency: None,
        reproducibility: signal_core::Reproducibility::BitExact,
    }
}

/// Emits a uniform random value per frame, and a trigger at offset 0.
#[derive(Default)]
struct Noise {
    initial: Option<RngStream>,
    rng: Option<RngStream>,
}

impl DirectPrimitive for Noise {
    fn describe() -> ModuleDescriptor {
        descriptor("test.noise", Feedthrough::Direct)
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.initial = Some(ctx.rng.clone());
        self.rng = Some(ctx.rng.clone());
    }

    fn reset(&mut self) {
        self.rng.clone_from(&self.initial);
    }

    fn process(&mut self, _inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        if let Some(rng) = &mut self.rng {
            for out in outputs.signal(0) {
                *out = rng.next_f64();
            }
        }
        if let Some(sink) = outputs.events(0) {
            sink.push(Event {
                offset: 0,
                payload: EventPayload::Trigger,
            });
        }
    }
}

/// Holds its last input.
#[derive(Default)]
struct Hold {
    state: f64,
}

impl StateOutputPrimitive for Hold {
    fn describe() -> ModuleDescriptor {
        descriptor("test.hold", Feedthrough::StateOutput)
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {}

    fn reset(&mut self) {
        self.state = 0.0;
    }

    fn emit(&self, outputs: &mut Outputs<'_>) {
        outputs.signal(0).fill(self.state);
    }

    fn absorb(&mut self, inputs: &Inputs<'_>) {
        if let Some(&x) = inputs.signal(0).last() {
            self.state = x;
        }
    }
}

fn ctx() -> PrepareContext {
    let address: InstanceAddress = "/noise".parse().unwrap();
    PrepareContext {
        sample_rate: 96_000.0,
        max_frames: 4,
        rng: RngStream::derive(1, &address),
        lent: None,
        generation: 0,
    }
}

#[test]
fn both_classes_are_object_safe() {
    let mut direct: Box<dyn DirectPrimitive> = Box::new(Noise::default());
    let mut state: Box<dyn StateOutputPrimitive> = Box::new(Hold::default());

    direct.prepare(&ctx());
    state.prepare(&ctx());

    let mut first = [0.0; 4];
    let mut sinks = [EventSink::with_capacity(1)];
    direct.process(
        &Inputs::new(4, &[0.0; 4], &[], &[], &[]),
        &mut Outputs::new(4, &mut first, &mut sinks),
    );
    assert!(first.iter().all(|x| (0.0..1.0).contains(x)));
    assert_eq!(sinks[0].events().len(), 1);

    // reset restarts the random stream; a full sink drops instead of panicking.
    direct.reset();
    let mut again = [0.0; 4];
    direct.process(
        &Inputs::new(4, &[0.0; 4], &[], &[], &[]),
        &mut Outputs::new(4, &mut again, &mut sinks),
    );
    assert_eq!(first, again);
    assert_eq!(sinks[0].overflows(), 1);

    let mut out = [f64::NAN];
    state.absorb(&Inputs::new(1, &[3.0], &[], &[], &[]));
    state.emit(&mut Outputs::new(1, &mut out, &mut []));
    assert_eq!(out, [3.0]);
    state.reset();
    state.emit(&mut Outputs::new(1, &mut out, &mut []));
    assert_eq!(out, [0.0]);
}

fn noise_type() -> ModuleType {
    ModuleType {
        descriptor: Noise::describe(),
        create: || Primitive::Direct(Box::new(Noise::default())),
    }
}

fn hold_type() -> ModuleType {
    ModuleType {
        descriptor: Hold::describe(),
        create: || Primitive::StateOutput(Box::new(Hold::default())),
    }
}

#[test]
fn registry_rejects_duplicate_names() {
    let mut registry = Registry::new();
    assert!(registry.is_empty());
    registry.register(noise_type()).unwrap();
    registry.register(hold_type()).unwrap();
    assert_eq!(
        registry.register(noise_type()),
        Err(RegistryError::DuplicateTypeName("test.noise"))
    );
    assert_eq!(registry.len(), 2);

    let names: Vec<_> = registry.iter().map(|t| t.descriptor.type_name).collect();
    assert_eq!(names, ["test.hold", "test.noise"]);

    let noise = registry.get("test.noise").unwrap();
    assert_eq!((noise.create)().feedthrough(), Feedthrough::Direct);
    let hold = registry.get("test.hold").unwrap();
    assert_eq!((hold.create)().feedthrough(), Feedthrough::StateOutput);
    assert_eq!(hold.descriptor.feedthrough, (hold.create)().feedthrough());
    assert!(registry.get("test.missing").is_none());
}
