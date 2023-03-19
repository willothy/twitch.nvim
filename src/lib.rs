use std::cell::RefCell;
use std::panic::PanicInfo;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use oxi::api::Buffer;
use oxi::conversion::ToObject;
use twitch_irc::login::StaticLoginCredentials;
use twitch_irc::message::{IRCPrefix, ServerMessage};
use twitch_irc::TwitchIRCClient;
use twitch_irc::{ClientConfig, SecureTCPTransport};

pub use anyhow::{anyhow, Context, Result};

use nvim_oxi::api::{self, opts::*, types::*, Window};
use nvim_oxi::{self as oxi, print, Dictionary, Function};

#[tokio::main]
pub async fn anon_read<N, C>(
    channel: N,
    callback: C,
    buf: Arc<Mutex<Option<Buffer>>>,
    signal: Receiver<()>,
) -> Result<()>
where
    N: Into<String>,
    C: Fn(ServerMessage, Arc<Mutex<Option<Buffer>>>) + Send + 'static,
{
    // default configuration is to join chat as anonymous.
    let config = ClientConfig::default();
    let (mut incoming_messages, client) =
        TwitchIRCClient::<SecureTCPTransport, StaticLoginCredentials>::new(config);

    // first thing you should do: start consuming incoming messages,
    // otherwise they will back up.
    let join_handle = tokio::spawn(async move {
        while let Some(message) = incoming_messages.recv().await {
            match &message {
                ServerMessage::Privmsg(_) => callback(message, buf.clone()),
                _ => {}
            }
            if signal.try_recv().is_ok() {
                return;
            }
            thread::sleep(Duration::from_millis(100))
        }
    });

    // join a channel
    // This function only returns an error if the passed channel login name is malformed,
    // so in this simple case where the channel name is hardcoded we can ignore the potential
    // error with `unwrap`.
    client.join(channel.into())?;

    // keep the tokio executor alive.
    // If you return instead of waiting the background task will exit.
    join_handle.await.context("Failed to wait for join handle")
}

fn display_msg(msg: ServerMessage) -> Option<String> {
    let source = msg.source();
    let prefix = source.prefix.clone().unwrap_or(IRCPrefix::HostOnly {
        host: "unknown".to_owned(),
    });
    let channel = source
        .params
        .get(0)
        .map(|s| s.to_string())
        .unwrap_or("unknown".to_owned());
    let name = match prefix {
        IRCPrefix::HostOnly { host } => host,
        IRCPrefix::Full { nick, user, host } => user.unwrap_or(nick),
    };
    if &*name == &*channel {
        return None;
    }
    let message: String = source
        .params
        .iter()
        .skip(1)
        .map(|s| s.to_owned())
        .collect::<Vec<String>>()
        .join(" ");

    Some(format!("{}: {}", name, message))
}

fn callback(msg: ServerMessage, buffer: Arc<Mutex<Option<Buffer>>>) {
    // api::notify(&display_msg(msg), LogLevel::Info, &NotifyOpts::default()).ok();
    let Some(msg) = display_msg(msg) else {
		return;
	};

    let Ok(mut buffer) = buffer.try_lock() else {
		api::err_writeln("No buffer for twitch");
    	return;
    };
    if buffer.is_none() {
        api::err_writeln("No buffer for twitch");
        return;
    }
    let Some(buffer) = buffer.as_mut() else {
		return;
	};
    let Ok(count) = buffer.line_count() else {
		api::err_writeln("No buffer line count for twitch");
    	return;
    };
    buffer.set_lines(count.., true, [msg]).ok();
    if count > 200 {
        buffer.set_lines(0..180, true, Vec::<String>::new()).ok();
    }
}

#[oxi::module]
fn twitch() -> oxi::Result<Dictionary> {
    // // buf.set_option("modifiable", false)?;
    // let win: Rc<RefCell<Option<Window>>> = Rc::default();
    //
    // let w = Rc::clone(&win);

    std::panic::set_hook(Box::new(|p: &PanicInfo| {
        api::notify(
            &*format!("{:?}", p.payload()),
            LogLevel::Error,
            &NotifyOpts::default(),
        )
        .ok();
    }));

    let buf = Arc::new(Mutex::new({
        let mut buf = api::create_buf(true, true)?;
        buf.set_name("twitch")?;
        // buf.set_option("modifiable", false)?;
        // buf.set_keymap(
        //     Mode::Normal,
        //     "q",
        //     ":bd",
        //     &SetKeymapOpts::builder().silent(true).build(),
        // )?;
        Some(buf)
    }));
    api::set_keymap(Mode::Normal, "q", "hello", &Default::default())?;
    let handle: Rc<RefCell<Option<JoinHandle<_>>>> = Rc::default();

    let tx = Arc::new(Mutex::new(None));
    let disconnect = Function::from_fn::<_, oxi::api::Error>({
        let handle = Rc::clone(&handle);
        let tx = Arc::clone(&tx);
        let buf = Arc::clone(&buf);
        move |()| -> Result<(), oxi::api::Error> {
            match buf.try_lock() {
                Ok(mut inner) => {
                    if let Some(buf) = inner.take() {
                        buf.delete(&BufDeleteOpts::default()).ok();
                    };
                }
                _ => {}
            }

            tx.try_lock()
                .map_err(|_| api::Error::Other("Lock".to_owned()))?
                .as_ref()
                .and_then(|s: &Sender<_>| -> Option<()> {
                    s.send(()).ok()?;
                    Some(())
                });

            if handle.borrow().as_ref().is_none() {
                api::err_writeln("Not connected");
                return Ok(());
            }

            let handle: JoinHandle<Result<_, _>> = handle
                .borrow_mut()
                .take()
                .ok_or(api::Error::Other("Join".to_owned()))?;
            handle.join().ok().map(|e: Result<(), _>| e.ok());
            Ok(())
        }
    });

    let connect = Function::from_fn::<_, oxi::Error>({
        let handle = Rc::clone(&handle);
        move |channel: String| {
            let buf = Arc::clone(&buf);
            // api::create_autocmd(
            //     ["BufDeletePre"],
            //     &CreateAutocmdOpts::builder()
            //         .callback({
            //             let handle = Rc::clone(&handle);
            //             move |args: AutocmdCallbackArgs| -> Result<bool, oxi::Error> {
            //                 if handle.borrow().as_ref().is_none() {
            //                     api::err_writeln("Not connected");
            //                     return Ok(true);
            //                 }
            //
            //                 let handle: JoinHandle<Result<_, _>> = handle
            //                     .borrow_mut()
            //                     .take()
            //                     .ok_or(oxi::Error::Api(api::Error::Other("Join".to_owned())))?;
            //                 let res = handle.join().ok().map(|e: Result<(), _>| e.ok());
            //                 Ok(true)
            //             }
            //         })
            //         .build(),
            // )?;

            let (sender, rx) = mpsc::channel();
            let mut tx = tx
                .try_lock()
                .map_err(|e| oxi::Error::Api(api::Error::Other(e.to_string())))?;
            tx.replace(sender);
            if handle.borrow().as_ref().is_none() {
                handle
                    .borrow_mut()
                    .replace(thread::spawn(|| anon_read(channel, callback, buf, rx)));
            }
            Ok(())
        }
    });
    // let open_window = Function::from_fn::<_, oxi::Error>({
    //     let handle = Rc::clone(&handle);
    //     move |channel: String| {
    //         if w.borrow().is_some() {
    //             api::err_writeln("Window is already open");
    //             return Ok(());
    //         }
    //
    //         let config = WindowConfig::builder()
    //             .relative(WindowRelativeTo::Editor)
    //             .height(5)
    //             .width(30)
    //             // .row(1)
    //             // .col(0)
    //             .build();
    //
    //         let mut win = w.borrow_mut();
    //         *win = Some(api::open_win(&buf, false, &config)?);
    //
    //         if handle.borrow().as_ref().is_none() {
    //             handle
    //                 .borrow_mut()
    //                 .replace(thread::spawn(|| anon_read(channel, callback)));
    //         }
    //
    //         Ok(())
    //     }
    // });
    //
    // let close_window = Function::from_fn({
    //     move |()| {
    //         if win.borrow().is_none() {
    //             api::err_writeln("Window is already closed");
    //             return Ok(());
    //         }
    //
    //         let win = win.borrow_mut().take().unwrap();
    //         win.close(false)
    //     }
    // });

    // let mut module = Dictionary::new();
    // module["open_window"] = open_window.to_object()?;
    // module["close_window"] = close_window.to_object()?;
    // // module["disconnect"] = disconnect.to_object()?;
    // Ok(module)
    Ok(Dictionary::from_iter([
        // ("open_window", open_window.to_object()?),
        // ("close_window", close_window.to_object()?),
        ("disconnect", disconnect.to_object()?),
        ("connect", connect.to_object()?),
    ]))
}
