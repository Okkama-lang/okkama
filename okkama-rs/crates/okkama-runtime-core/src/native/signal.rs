//! `runtime/signal`: наблюдение за сигналами процесса (TERM, INT, HUP), #80,
//! #638. Обработчик пишет номер сигнала в self-pipe (единственное, что можно
//! делать в обработчике); поток-диспетчер читает трубу и считает приходы
//! каждого сигнала. Sparrow берёт этот native через мост (`native_origin`).
//!
//! Наблюдение (`signal_watch`) — ожидание без потока: оно держит обработчики
//! своих сигналов, пока не сработает. Первый приход одного из них
//! запоминается (`signal_arrived` отвечает его имя, `signal_arrival` будит
//! ждущих), и наблюдение отпускает обработчики: сигналу, которого больше никто
//! не ждёт, возвращается прежнее действие, так что второй завершает процесс.
#![allow(unsafe_code)]

use std::rc::Rc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard, OnceLock, PoisonError};

use super::Error;
use super::support::{int, int_value, items, small, text};
use crate::failure::{Failure, Kind};
use crate::sched;
use crate::{Value, abi::Native};

/// Сигналы, которые можно ждать, и их номера.
const SIGNALS: [(&str, libc::c_int); 3] = [("TERM", libc::SIGTERM), ("INT", libc::SIGINT), ("HUP", libc::SIGHUP)];

/// Пишущий конец трубы (`-1`, пока диспетчер не запущен).
static WRITE_END: AtomicI32 = AtomicI32::new(-1);

struct Registry {
    /// Сколько раз пришёл каждый сигнал (по месту в `SIGNALS`).
    arrived: [u64; 3],
    /// Прежние действия сигналов, которым поставлен обработчик.
    prior: [Option<libc::sigaction>; 3],
    /// Идущие ожидания: номер и места ожидаемых сигналов.
    waiters: Vec<(u64, Vec<usize>)>,
    /// Наблюдения (#638): номер, места сигналов и сработавший сигнал. Пока не
    /// сработало, наблюдение — ещё и ожидание в `waiters`.
    watches: Vec<(u64, Vec<usize>, Option<usize>)>,
    /// Идущие `signal_arrival` (#940): номер ожидания. Отмена убирает номер, и
    /// поток ожидания выходит, не дожидаясь сигнала.
    arrivals: Vec<u64>,
    next: u64,
    started: bool,
}

struct Hub {
    registry: Mutex<Registry>,
    changed: Condvar,
}

fn hub() -> &'static Hub {
    static HUB: OnceLock<Hub> = OnceLock::new();
    HUB.get_or_init(|| Hub {
        registry: Mutex::new(Registry {
            arrived: [0; 3],
            prior: [None; 3],
            waiters: Vec::new(),
            watches: Vec::new(),
            arrivals: Vec::new(),
            next: 0,
            started: false,
        }),
        changed: Condvar::new(),
    })
}

fn registry() -> MutexGuard<'static, Registry> {
    hub().registry.lock().unwrap_or_else(PoisonError::into_inner)
}

extern "C" fn on_signal(number: libc::c_int) {
    let fd = WRITE_END.load(Ordering::SeqCst);
    if fd >= 0 {
        let byte = number as u8;
        // SAFETY: write(2) безопасен в обработчике сигнала; буфер — один байт,
        // запись неблокирующая (полная труба теряет лишний байт, не процесс).
        unsafe {
            let saved = *errno();
            libc::write(fd, &byte as *const u8 as *const libc::c_void, 1);
            *errno() = saved;
        }
    }
}

/// Место errno текущего потока: обработчик не должен его менять.
#[cfg(any(target_os = "linux", target_os = "android"))]
pub(super) unsafe fn errno() -> *mut libc::c_int {
    // SAFETY: errno текущего потока.
    unsafe { libc::__errno_location() }
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
pub(super) unsafe fn errno() -> *mut libc::c_int {
    // SAFETY: errno текущего потока.
    unsafe { libc::__error() }
}

/// Труба и поток-диспетчер (один раз на процесс).
fn start(registry: &mut Registry) -> bool {
    if registry.started {
        return true;
    }
    let mut ends = [0 as libc::c_int; 2];
    // SAFETY: pipe(2) заполняет два дескриптора.
    if unsafe { libc::pipe(ends.as_mut_ptr()) } != 0 {
        return false;
    }
    // SAFETY: свои дескрипторы: не наследуются потомками; запись не блокирует
    // обработчик сигнала.
    unsafe {
        libc::fcntl(ends[0], libc::F_SETFD, libc::FD_CLOEXEC);
        libc::fcntl(ends[1], libc::F_SETFD, libc::FD_CLOEXEC);
        let flags = libc::fcntl(ends[1], libc::F_GETFL);
        libc::fcntl(ends[1], libc::F_SETFL, flags | libc::O_NONBLOCK);
    }
    let read_end = ends[0];
    let spawned = std::thread::Builder::new()
        .name("okkama-signal".into())
        .spawn(move || dispatch(read_end));
    if spawned.is_err() {
        // SAFETY: свои дескрипторы, ещё никому не отданы.
        unsafe {
            libc::close(ends[0]);
            libc::close(ends[1]);
        }
        return false;
    }
    WRITE_END.store(ends[1], Ordering::SeqCst);
    registry.started = true;
    true
}

/// Поток-диспетчер: приход сигнала увеличивает его счётчик и будит ожидающих.
fn dispatch(read_end: libc::c_int) {
    loop {
        let mut byte = 0u8;
        // SAFETY: свой конец трубы и буфер в один байт.
        let count = unsafe { libc::read(read_end, &mut byte as *mut u8 as *mut libc::c_void, 1) };
        if count == 1 {
            if let Some(place) = SIGNALS.iter().position(|(_, number)| *number == libc::c_int::from(byte)) {
                let mut registry = registry();
                registry.arrived[place] += 1;
                fire(&mut registry, place);
                drop(registry);
                hub().changed.notify_all();
            }
        } else if count == 0 || std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            return;
        }
    }
}

/// Сигнал `place` пришёл: наблюдения, которые его ждали, срабатывают и
/// перестают держать обработчики (второй такой сигнал — прежнее действие).
fn fire(registry: &mut Registry, place: usize) {
    let mut fired = Vec::new();
    for (id, wanted, arrived) in registry.watches.iter_mut() {
        if arrived.is_none() && wanted.contains(&place) {
            *arrived = Some(place);
            fired.push(*id);
        }
    }
    if !fired.is_empty() {
        registry.waiters.retain(|(waiter, _)| !fired.contains(waiter));
        release(registry);
    }
}

/// Наблюдение за сигналами `wanted` (#638): номер. Обработчики ставятся тем,
/// у кого их ещё нет, и наблюдение записано в той же критической секции, так
/// что сигнал, который диспетчер обработает сразу после установки, его уже
/// застанет (#940).
fn watch(wanted: &[usize]) -> Option<u64> {
    let mut registry = registry();
    if !start(&mut registry) {
        return None;
    }
    for &place in wanted {
        if registry.prior[place].is_some() {
            continue;
        }
        // SAFETY: обработчик — extern "C" функция без состояния; прежнее
        // действие сохраняется, чтобы вернуть его.
        let prior = unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = on_signal as *const () as usize;
            action.sa_flags = libc::SA_RESTART;
            libc::sigemptyset(&mut action.sa_mask);
            let mut prior: libc::sigaction = std::mem::zeroed();
            (libc::sigaction(SIGNALS[place].1, &action, &mut prior) == 0).then_some(prior)
        };
        let Some(prior) = prior else {
            release(&mut registry);
            return None;
        };
        registry.prior[place] = Some(prior);
    }
    let id = registry.next;
    registry.next += 1;
    registry.waiters.push((id, wanted.to_vec()));
    registry.watches.push((id, wanted.to_vec(), None));
    Some(id)
}

/// Снять ожидание `id` (повторный вызов ничего не делает).
fn leave(registry: &mut Registry, id: u64) {
    registry.waiters.retain(|(waiter, _)| *waiter != id);
    release(registry);
}

/// Вернуть прежнее действие сигналам, которых больше никто не ждёт.
fn release(registry: &mut Registry) {
    for (place, (_, number)) in SIGNALS.iter().enumerate() {
        if registry.waiters.iter().any(|(_, wanted)| wanted.contains(&place)) {
            continue;
        }
        if let Some(prior) = registry.prior[place].take() {
            // SAFETY: действие, сохранённое sigaction(2) при установке.
            unsafe {
                libc::sigaction(*number, &prior, std::ptr::null_mut());
            }
        }
    }
}

pub fn handles(native: Native) -> bool {
    native.module() == "runtime/signal"
}

fn refusal(native: Native, code: &str, message: &str) -> Error {
    Error::Failure(crate::abi::NativeFailure {
        domain: "core.signal".into(),
        code: code.into(),
        operation: native.symbol().into(),
        message: message.into(),
    })
}

/// Места названных сигналов, без повторов; `Err` — сообщение отказа.
fn places(value: &Value) -> Result<Result<Vec<usize>, &'static str>, Error> {
    let mut wanted = Vec::new();
    for item in items(value)?.iter() {
        let name = item.as_string()?;
        let Some(place) = SIGNALS.iter().position(|(known, _)| *known == name) else {
            return Ok(Err("Unknown signal name"));
        };
        if !wanted.contains(&place) {
            wanted.push(place);
        }
    }
    if wanted.is_empty() {
        return Ok(Err("No signal to wait for"));
    }
    Ok(Ok(wanted))
}

/// Номер наблюдения из аргумента.
fn watch_id(value: &Value) -> Result<u64, Error> {
    small(int(value)?, usize::MAX).map(|id| id as u64).ok_or_else(|| Error::InvalidState("signal watch".into()))
}

/// Имя сработавшего сигнала наблюдения `id`, `""` — ещё не сработало или
/// наблюдения нет.
fn arrived(id: u64) -> &'static str {
    registry()
        .watches
        .iter()
        .find(|(watch, _, _)| *watch == id)
        .and_then(|(_, _, arrived)| *arrived)
        .map_or("", |place| SIGNALS[place].0)
}

/// Снять наблюдение `id` (повторно — ничего).
fn unwatch(id: u64) {
    let mut registry = registry();
    registry.watches.retain(|(watch, _, _)| *watch != id);
    leave(&mut registry, id);
    drop(registry);
    hub().changed.notify_all();
}

/// Рабочий поток `signal_arrival`: ждать, пока наблюдение `id` сработает;
/// `None` — его сняли.
#[cfg(test)]
fn await_watch(id: u64) -> Option<usize> {
    await_watch_for(id, None)
}

/// [`await_watch`] for the arrival `token`: `None` too once the arrival is
/// cancelled (its token removed).
fn await_watch_for(id: u64, token: Option<u64>) -> Option<usize> {
    let mut registry = registry();
    loop {
        if let Some(token) = token
            && !registry.arrivals.contains(&token)
        {
            return None;
        }
        match registry.watches.iter().find(|(watch, _, _)| *watch == id) {
            None => return None,
            Some((_, _, Some(place))) => return Some(*place),
            Some(_) => {}
        }
        registry = hub().changed.wait(registry).unwrap_or_else(PoisonError::into_inner);
    }
}

/// What a finished `signal_arrival` wait settles its promise with: the name
/// of the signal that came, or `already_closed` when the watch was taken off
/// under it (#1013; the promise waited forever before). A cancelled arrival
/// also ends with `None`, but its promise is settled already, and `settle`
/// leaves it as it is.
fn arrival_outcome(native: Native, received: Option<usize>) -> sched::Outcome {
    match received {
        Some(place) => Ok(text(SIGNALS[place].0)),
        None => Err(Failure::new(Kind::Native(Rc::new(crate::abi::NativeFailure {
            domain: "core.signal".into(),
            code: "already_closed".into(),
            operation: native.symbol().into(),
            message: "Signal watch is closed".into(),
        })))),
    }
}

pub fn execute(native: Native, args: &[Value]) -> Result<Value, Error> {
    match native {
        Native::SignalSignalWatch => {
            let wanted = match places(&args[0])? {
                Ok(wanted) => wanted,
                Err(message) => return Err(refusal(native, "invalid_input", message)),
            };
            let Some(id) = watch(&wanted) else {
                return Err(refusal(native, "unsupported", "Signals cannot be handled here"));
            };
            Ok(int_value(id as i64))
        }
        Native::SignalSignalArrived => Ok(text(arrived(watch_id(&args[0])?))),
        Native::SignalSignalUnwatch => {
            unwatch(watch_id(&args[0])?);
            Ok(Value::Bool(true))
        }
        Native::SignalSignalArrival => {
            let id = watch_id(&args[0])?;
            if let Some(place) = registry().watches.iter().find(|(watch, _, _)| *watch == id).and_then(|w| w.2) {
                return Ok(Value::Future(sched::resolved(Ok(text(SIGNALS[place].0)))));
            }
            let token = {
                let mut registry = registry();
                let token = registry.next;
                registry.next += 1;
                registry.arrivals.push(token);
                token
            };
            let promise = sched::promise();
            let target = promise.clone();
            let op = sched::offload(
                move || await_watch_for(id, Some(token)),
                move |received: Option<usize>| sched::settle(&target, arrival_outcome(native, received)),
            );
            // Исход или отмена (#940): ожидание снимается (наблюдение остаётся,
            // его снимает unwatch), и поток будят, чтобы он вышел сразу.
            promise.listen(move || {
                op.detach();
                registry().arrivals.retain(|&waiting| waiting != token);
                hub().changed.notify_all();
            });
            Ok(Value::Future(promise))
        }
        _ => Err(Error::InvalidState("not a signal native".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #638: a watch keeps its signal's handler until the signal comes; then it
    /// remembers that signal, lets the handler go (the earlier action is back,
    /// so a second one ends the process), and unwatching forgets it.
    #[test]
    fn a_watch_fires_once_and_lets_its_handler_go() {
        let hup = 2;
        let id = watch(&[hup]).expect("a watch");
        assert_eq!(arrived(id), "");
        assert!(registry().prior[hup].is_some(), "the handler is installed while watching");
        {
            let mut registry = registry();
            registry.arrived[hup] += 1;
            fire(&mut registry, hup);
            assert!(!registry.waiters.iter().any(|(waiter, _)| *waiter == id));
            assert!(registry.prior[hup].is_none(), "the earlier action is back");
        }
        assert_eq!(arrived(id), "HUP");
        assert_eq!(await_watch(id), Some(hup));
        unwatch(id);
        assert_eq!(arrived(id), "");
        assert_eq!(await_watch(id), None);
    }

    /// #940: the watch is in the registry by the time the handlers are, so a
    /// signal the dispatcher handles at once fires it.
    #[test]
    fn a_watch_is_recorded_with_its_handlers() {
        let term = 0;
        let id = watch(&[term]).expect("a watch");
        {
            let mut registry = registry();
            assert!(registry.watches.iter().any(|(watch, _, _)| *watch == id));
            registry.arrived[term] += 1;
            fire(&mut registry, term);
        }
        assert_eq!(arrived(id), "TERM");
        unwatch(id);
    }

    /// #1013: unwatching under a waiting arrival ends its wait, and the
    /// arrival is refused with `already_closed` instead of never settling.
    /// INT: the other tests fire HUP and TERM watches, which would fire this one.
    #[test]
    fn an_arrival_under_an_unwatch_is_refused() {
        let watched = execute(Native::SignalSignalWatch, &[super::super::support::array(vec![text("INT")])]);
        let Ok(id) = watched else { panic!("a watch") };
        let Ok(Value::Future(arrival)) = execute(Native::SignalSignalArrival, std::slice::from_ref(&id)) else {
            panic!("an arrival future")
        };
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert!(arrival.running());
        assert!(execute(Native::SignalSignalUnwatch, std::slice::from_ref(&id)).is_ok());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while arrival.running() && std::time::Instant::now() < deadline {
            sched::pump();
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let failure = arrival.failure().expect("the arrival is refused, not left waiting");
        let Kind::Native(native) = &failure.kind else { panic!("a native failure") };
        assert_eq!(native.code, "already_closed");
        let Ok(Value::Str(name)) = arrival_outcome(Native::SignalSignalArrival, Some(2)) else { panic!("a name") };
        assert_eq!(&*name, "HUP");
    }

    /// #940: a cancelled arrival (its token gone) stops waiting although the
    /// watch has not fired; the watch stays.
    #[test]
    fn a_cancelled_arrival_stops_waiting() {
        let int = 1;
        let id = watch(&[int]).expect("a watch");
        let token = {
            let mut registry = registry();
            let token = registry.next;
            registry.next += 1;
            registry.arrivals.push(token);
            token
        };
        let waiting = std::thread::spawn(move || await_watch_for(id, Some(token)));
        std::thread::sleep(std::time::Duration::from_millis(20));
        registry().arrivals.retain(|&waiting| waiting != token);
        hub().changed.notify_all();
        assert_eq!(waiting.join().expect("the waiting thread"), None);
        assert!(registry().watches.iter().any(|(watch, _, _)| *watch == id));
        unwatch(id);
    }
}
