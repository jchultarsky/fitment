//! Roles (docs/research/m6-api.md, AU5, M6-17): reader, writer, admin.
//!
//! Ported from Vectera's `commons/src/auth.rs` (MIT, © 2024 Julian
//! Chultarsky, the same author): the three-level hierarchy and its
//! `permits` table, renamed from Vectera's viewer, editor and admin to
//! M6-17's reader, writer and admin, and asked of an operation's least role
//! (`operations::Role`) instead of a coarse permission.

use serde::{Deserialize, Serialize};

use crate::http::operations::Role as Needed;

/// A user's or a token's role (AU5), ordered `reader < writer < admin`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Reads the catalog and works on the caller's own resources.
    Reader,
    /// Also changes the catalog.
    Writer,
    /// Also maintains it and manages users.
    Admin,
}

impl Role {
    /// Every role, lowest first.
    pub const ALL: [Self; 3] = [Self::Reader, Self::Writer, Self::Admin];

    /// The name `server.db` and the documents write.
    pub fn name(self) -> &'static str {
        match self {
            Self::Reader => "reader",
            Self::Writer => "writer",
            Self::Admin => "admin",
        }
    }

    /// The role of `name`, exactly as written.
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|r| r.name() == name)
    }

    /// True if this role may run an operation that needs `needed` (AU5):
    /// Vectera's `permits` table over M6's three roles.
    pub fn permits(self, needed: Needed) -> bool {
        matches!(
            (self, needed),
            (_, Needed::None)
                | (Self::Admin, _)
                | (Self::Writer, Needed::Reader | Needed::Writer)
                | (Self::Reader, Needed::Reader)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// AU5: Vectera's matrix, ported: each role permits what it and every
    /// role below it may do, and nothing above.
    #[test]
    fn role_permits_matrix() {
        use Needed as N;
        let expect = [
            (Role::Reader, [true, true, false, false]),
            (Role::Writer, [true, true, true, false]),
            (Role::Admin, [true, true, true, true]),
        ];
        for (role, row) in expect {
            for (needed, want) in [N::None, N::Reader, N::Writer, N::Admin]
                .into_iter()
                .zip(row)
            {
                assert_eq!(role.permits(needed), want, "{role:?} {needed:?}");
            }
        }
        assert!(Role::Reader < Role::Writer && Role::Writer < Role::Admin);
    }

    #[test]
    fn role_round_trip_via_its_name() {
        for role in Role::ALL {
            assert_eq!(Role::parse(role.name()), Some(role));
        }
        assert_eq!(Role::parse("ADMIN"), None, "names are exact");
        assert_eq!(Role::parse("viewer"), None);
    }
}
