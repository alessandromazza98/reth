use crate::runner::CliContext;
use clap::Parser;
use reth_db::{open_db_read_only, tables};
use reth_primitives::ChainSpecBuilder;
use reth_provider::{DatabaseProviderRO, ProviderFactory};
use reth_revm::interpreter::{opcode, OpCode};
use std::{collections::HashMap, path::PathBuf, sync::Arc};
use tracing::info;

/// `reth count-opcodes` command
#[derive(Debug, Parser)]
pub struct Command {
    #[arg(long, value_name = "DB_DIR", verbatim_doc_comment)]
    db_dir: PathBuf,
}

impl Command {
    /// Execute `count-opcodes` command
    pub async fn execute(&self) -> eyre::Result<()> {
        // read db
        let db = Arc::new(open_db_read_only(self.db_dir.as_path(), None)?);
        // create spec
        let spec = Arc::new(ChainSpecBuilder::mainnet().build());
        // create db provider
        let factory = ProviderFactory::new(db.clone(), spec.clone());
        let provider = factory.provider()?;

        // get bytecodes table
        let bytecodes = provider.table::<tables::Bytecodes>()?;

        // create hashmap
        let mut opcodes: HashMap<u8, usize> = HashMap::new();
        info!("start opcodes processing...");
        for (address, bytecode) in bytecodes {
            let bytes = bytecode.bytes();
            let range = bytes.as_ptr_range();
            let start = range.start;
            let mut iterator = start;
            let end = range.end;
            while iterator < end {
                let opcode = unsafe { *iterator };
                if opcode >= opcode::PUSH1 && opcode <= opcode::PUSH32 {
                    // it's a PUSH opcode
                    *opcodes.entry(opcode).or_insert(1) += 1;
                    let push_offset = opcode.wrapping_sub(opcode::PUSH1);
                    // SAFETY: iterator access range is checked in the while loop
                    iterator = unsafe { iterator.offset((push_offset + 2) as isize) };
                } else {
                    // not a PUSH opcode
                    *opcodes.entry(opcode).or_insert(1) += 1;
                    // SAFETY: iterator access range is checked in the while loop
                    iterator = unsafe { iterator.offset(1) };
                }
            }
        }
        info!("opcodes processing done!");
        info!("start opcodes printing...");
        for (opcode, occurencies) in opcodes {
            match OpCode::new(opcode) {
                Some(op) => info!("{}: {}", op, occurencies),
                None => info!("{}: {}", opcode, occurencies),
            };
        }
        info!("opcodes printing done!");
        Ok(())
    }
}
