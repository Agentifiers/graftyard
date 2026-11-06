//! Fold blobs that cross releases (`design/SHARED.md`, "Record formats and the two-release rule").
//!
//! The identity folds are stored whole and sent whole in `graftyard_raft::codec`: as a member's `fold` row, as a
//! `dep` read for another group's `decide`, and inside the edge's `HostedDir`. A field added to a fold would change all
//! of those blobs at once, so a release that adds one could not read the release before it, nor be read by it, while a
//! site day rolls one site a day. Milestone 3's rows therefore sit in a trailing `more` part, written only when it
//! holds a row. A release writes such a row only from an event of a format that `FormatsEnable` has enabled, which
//! happens only once every member reads it, so until then a fold's bytes are the old release's bytes. A blob with no
//! trailing part reads as an empty one.

/// Implements `Serialize` and `Deserialize` for a fold as its base fields in order, which is the byte form a derived
/// struct had, then, only when `more` is not its default, a `1` and `more`. Decoding reads the base fields and then
/// `more` if a marker follows. Postcard fails a `u8` read only at the end of its input, so the one error ignored is
/// the end of a blob an older release wrote; an error inside `more` is returned.
macro_rules! tail_serde {
    ($fold:ident { $($f:ident: $t:ty),* $(,)? } more: $more:ty) => {
        impl serde::Serialize for $fold {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                use serde::ser::SerializeTuple;
                let more = self.more != <$more>::default();
                let n = [$(stringify!($f)),*].len();
                let mut t = s.serialize_tuple(n + if more { 2 } else { 0 })?;
                $(t.serialize_element(&self.$f)?;)*
                if more {
                    t.serialize_element(&1u8)?;
                    t.serialize_element(&self.more)?;
                }
                t.end()
            }
        }

        impl<'de> serde::Deserialize<'de> for $fold {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                struct V;
                impl<'de> serde::de::Visitor<'de> for V {
                    type Value = $fold;
                    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        f.write_str(stringify!($fold))
                    }
                    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<$fold, A::Error> {
                        $(
                            let $f: $t = seq
                                .next_element()?
                                .ok_or_else(|| serde::de::Error::missing_field(stringify!($f)))?;
                        )*
                        let more: $more = match seq.next_element::<u8>() {
                            Ok(Some(1)) => {
                                seq.next_element()?.ok_or_else(|| serde::de::Error::missing_field("more"))?
                            }
                            Ok(Some(m)) => {
                                return Err(serde::de::Error::invalid_value(
                                    serde::de::Unexpected::Unsigned(u64::from(m)),
                                    &"1, the marker of a fold's trailing part",
                                ));
                            }
                            Ok(None) | Err(_) => <$more>::default(),
                        };
                        Ok($fold { $($f,)* more })
                    }
                }
                let n = [$(stringify!($f)),*].len();
                d.deserialize_tuple(n + 2, V)
            }
        }
    };
}

pub(crate) use tail_serde;
