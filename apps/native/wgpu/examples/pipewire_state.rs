//! Private smoke observer: node state and client properties, never parameters.
//! pw-dump's async parameter results can prematurely complete buffer allocation
//! in PipeWire 1.0.5. Observe the real graph without issuing those requests.
#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("The PipeWire state observer is Linux-only.");
    std::process::exit(3);
}

#[cfg(target_os = "linux")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use pipewire::{self as pw, proxy::Listener, proxy::ProxyT, types::ObjectType};
    use serde_json::{Map, Value, json};
    use std::{
        cell::{Cell, RefCell},
        collections::BTreeMap,
        rc::Rc,
        time::{Duration, Instant},
    };

    fn properties(dict: Option<&pw::spa::utils::dict::DictRef>) -> Map<String, Value> {
        dict.into_iter()
            .flat_map(|dict| dict.iter())
            .map(|(key, value)| (key.into(), Value::String(value.into())))
            .collect()
    }

    type Observed = (Box<dyn Listener>, Box<dyn ProxyT>);
    pw::init();
    let main_loop = pw::main_loop::MainLoopRc::new(None)?;
    let context = pw::context::ContextRc::new(&main_loop, None)?;
    let core = context.connect_rc(None)?;
    let registry = core.get_registry_rc()?;
    let pending = Rc::new(Cell::new(core.sync(0)?));
    let finished = Rc::new(Cell::new(false));
    let failure = Rc::new(RefCell::new(None::<String>));
    let _core_listener = core
        .add_listener_local()
        .done({
            let pending = pending.clone();
            let finished = finished.clone();
            move |id, seq| {
                if id == pw::core::PW_ID_CORE && seq == pending.get() {
                    finished.set(true);
                }
            }
        })
        .error({
            let failure = failure.clone();
            move |id, seq, result, message| {
                *failure.borrow_mut() = Some(format!("PipeWire {id}/{seq}: {result}: {message}"));
            }
        })
        .register();

    // Keep listeners before proxies in each tuple so they unregister first.
    let observed = Rc::new(RefCell::new(BTreeMap::<u32, Observed>::new()));
    let _registry_listener = registry
        .add_listener_local()
        .global({
            let registry = registry.downgrade();
            let core = core.downgrade();
            let observed = observed.clone();
            let failure = failure.clone();
            let finished = finished.clone();
            move |global| {
                let (Some(registry), Some(core)) = (registry.upgrade(), core.upgrade()) else {
                    return;
                };
                let result = (|| -> Result<(), pw::Error> {
                    let id = global.id;
                    let bound: Observed = match global.type_ {
                        ObjectType::Node => {
                            let node: pw::node::Node = registry.bind(global)?;
                            let props = RefCell::new(properties(global.props));
                            let listener = node
                                .add_listener_local()
                                .info(move |info| {
                                    if let Some(update) = info.props() {
                                        *props.borrow_mut() = properties(Some(update));
                                    }
                                    let state = match info.state() {
                                        pw::node::NodeState::Running => "running",
                                        pw::node::NodeState::Idle => "idle",
                                        pw::node::NodeState::Suspended => "suspended",
                                        pw::node::NodeState::Creating => "creating",
                                        pw::node::NodeState::Error(_) => "error",
                                    };
                                    println!(
                                        "{}",
                                        json!([{"id": id, "info": {
                                            "state": state, "props": *props.borrow()
                                        }}])
                                    );
                                })
                                .register();
                            (Box::new(listener), Box::new(node))
                        }
                        ObjectType::Client => {
                            let client: pw::client::Client = registry.bind(global)?;
                            let listener = client
                                .add_listener_local()
                                .info(move |info| {
                                    if info.props().is_some() {
                                        println!(
                                            "{}",
                                            json!([{"id": id, "info": {
                                                "props": properties(info.props())
                                            }}])
                                        );
                                    }
                                })
                                .register();
                            (Box::new(listener), Box::new(client))
                        }
                        _ => return Ok(()),
                    };
                    observed.borrow_mut().insert(id, bound);
                    // Wait for this bind's initial info, not merely the registry.
                    pending.set(core.sync(0)?);
                    finished.set(false);
                    Ok(())
                })();
                if let Err(error) = result {
                    *failure.borrow_mut() = Some(error.to_string());
                }
            }
        })
        .global_remove(move |id| {
            observed.borrow_mut().remove(&id);
            println!("{}", json!([{"id": id, "info": null}]));
        })
        .register();

    println!("[]");
    let deadline = Instant::now() + Duration::from_secs(2);
    while !finished.get() && failure.borrow().is_none() && Instant::now() < deadline {
        main_loop
            .loop_()
            .iterate(pw::loop_::Timeout::Finite(Duration::from_millis(25)));
    }
    if let Some(error) = failure.borrow_mut().take() {
        return Err(error.into());
    }
    if !finished.get() {
        return Err("PipeWire state snapshot timed out".into());
    }
    Ok(())
}
