// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Identity resolution and deterministic options builder for `org update add-id`.

use std::collections::HashSet;

use org_tools_core::id::{mint_random_uuid_v4, IdGenerator};
use org_tools_core::property::{uuid_v5, DeterministicIdConfig, DeterministicIdOptions};

use super::AddIdArgs;
use crate::EXIT_ERROR;

/// Generates a random UUIDv4 value checked for collision against `seen`.
pub(crate) fn mint_collision_checked_uuidv4(seen: &mut HashSet<String>) -> String {
    loop {
        let cand = mint_random_uuid_v4();
        if seen.insert(cand.clone()) {
            return cand;
        }
    }
}

/// Mints an identity pair `(id, id_seed)` according to configured arguments.
pub(crate) fn resolve_target_identity(
    args: &AddIdArgs,
    seen: &mut HashSet<String>,
) -> Result<(String, Option<String>), i32> {
    if let Some(ref urn) = args.uuidv5_seed {
        let cfg = DeterministicIdConfig::resolve(args.namespace.as_deref(), None).map_err(|e| {
            eprintln!("org: {e}");
            EXIT_ERROR
        })?;
        let id = uuid_v5(&cfg.namespace, urn).to_string();
        let seed = if args.write_seed {
            Some(urn.clone())
        } else {
            None
        };
        return Ok((id, seed));
    }
    Ok((mint_collision_checked_uuidv4(seen), None))
}

/// Selects the classic [`IdGenerator`] configured on [`AddIdArgs`].
pub(crate) fn build_id_generator(args: &AddIdArgs) -> IdGenerator {
    if let Some(ref tpl) = args.id_format {
        IdGenerator::Template(tpl.clone())
    } else if let Some(ref cmd) = args.id_command {
        IdGenerator::Command(cmd.clone())
    } else {
        IdGenerator::Uuid
    }
}

/// Resolves optional [`DeterministicIdOptions`] from [`AddIdArgs`].
pub(crate) fn build_deterministic_opts(
    args: &AddIdArgs,
) -> Result<Option<DeterministicIdOptions>, i32> {
    if !args.deterministic {
        return Ok(None);
    }
    let cfg = DeterministicIdConfig::resolve(args.namespace.as_deref(), args.urn_prefix.as_deref())
        .map_err(|e| {
            eprintln!("org: {e}");
            EXIT_ERROR
        })?;
    Ok(Some(DeterministicIdOptions {
        config: cfg,
        explicit_seed: args.seed.clone(),
        scope: args.scope.clone(),
        kind: args.kind.clone(),
        author: args.author.clone(),
    }))
}
