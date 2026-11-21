mod db;
mod environment;
mod http;
mod network;
mod monitor;
mod proxy;
mod deploy;
mod git;
mod backup;
mod settings;
mod tunnel;
pub mod secrets;
pub mod secret_store_acl;

use std::{collections::{HashMap, HashSet}, env, net::SocketAddr, path::PathBuf, process::Child, sync::{Arc, Mutex}, time::Duration};
use rusqlite::Connection;
