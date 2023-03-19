use twitch_irc::message::ServerMessage;

pub fn callback(msg: ServerMessage) {
    println!("{msg:?}")
}

fn main() -> twitch::Result<()> {
    twitch::anon_read("melkey", callback)
}
