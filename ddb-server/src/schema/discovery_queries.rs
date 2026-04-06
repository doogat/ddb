use async_graphql::dynamic::*;
use async_graphql::{Name, Value as GqlValue};
use indexmap::IndexMap;

use crate::error::to_server_error;
use crate::read_pool::ReadPool;

use super::base_types::simple_field;

/// Types produced by discovery query registration that must be registered on
/// the schema alongside the Query object.
pub(crate) struct DiscoveryOutput {
    pub query: Object,
    pub sequence_node_type: Object,
    pub sequence_info_type: Object,
    pub broken_sequence_type: Object,
}

/// Register discovery-focused query fields (orphans, unlinked mentions,
/// sequences, suggestions, stale doogats) onto `query` and return the
/// auxiliary types that the caller must register on the schema.
pub(crate) fn register_discovery_fields(mut query: Object) -> DiscoveryOutput {
    // unlinkedMentions(id: ID!): [UnlinkedMention!]!
    {
        query = query.field(
            Field::new(
                "unlinkedMentions",
                TypeRef::named_nn_list_nn("UnlinkedMention"),
                |ctx| {
                    FieldFuture::new(async move {
                        let pool = ctx.data::<ReadPool>()?;
                        let id = ctx.args.try_get("id")?.string()?.to_string();
                        let mentions = pool.unlinked_mentions(id).await.map_err(to_server_error)?;
                        Ok(Some(FieldValue::list(mentions.iter().map(|m| {
                            let mut obj = IndexMap::new();
                            obj.insert(Name::new("sourceId"), GqlValue::from(m.source_id.as_str()));
                            obj.insert(
                                Name::new("sourceTitle"),
                                GqlValue::from(m.source_title.as_str()),
                            );
                            obj.insert(Name::new("snippet"), GqlValue::from(m.snippet.as_str()));
                            FieldValue::owned_any(GqlValue::Object(obj))
                        }))))
                    })
                },
            )
            .argument(InputValue::new("id", TypeRef::named_nn(TypeRef::ID)).description("The doogat ID to find unlinked mentions for."))
            .description("Find doogats that mention this doogat's title as plain text without a wikilink."),
        );
    }

    // suggestions(id: ID!, limit: Int): [Suggestion!]!
    {
        query = query.field(
            Field::new(
                "suggestions",
                TypeRef::named_nn_list_nn("Suggestion"),
                |ctx| {
                    FieldFuture::new(async move {
                        let pool = ctx.data::<ReadPool>()?;
                        let id = ctx.args.try_get("id")?.string()?.to_string();
                        let limit = ctx
                            .args
                            .get("limit")
                            .and_then(|v| v.i64().ok())
                            .unwrap_or(10) as usize;
                        let suggestions = pool
                            .suggest_links(id, limit)
                            .await
                            .map_err(to_server_error)?;
                        Ok(Some(FieldValue::list(suggestions.iter().map(|s| {
                            let tags: Vec<GqlValue> = s
                                .shared_tags
                                .iter()
                                .map(|t| GqlValue::from(t.as_str()))
                                .collect();
                            let mut obj = IndexMap::new();
                            obj.insert(Name::new("id"), GqlValue::from(s.id.as_str()));
                            obj.insert(Name::new("title"), GqlValue::from(s.title.as_str()));
                            obj.insert(Name::new("score"), GqlValue::from(s.score));
                            obj.insert(Name::new("sharedTags"), GqlValue::List(tags));
                            FieldValue::owned_any(GqlValue::Object(obj))
                        }))))
                    })
                },
            )
            .argument(InputValue::new("id", TypeRef::named_nn(TypeRef::ID)).description("The doogat ID to get link suggestions for."))
            .argument(InputValue::new("limit", TypeRef::named(TypeRef::INT)).description("Maximum suggestions to return. Default 10."))
            .description("Suggest doogats to link based on shared tags and content similarity."),
        );
    }

    // staleDoogats(type: String): [StaleDoogat!]!
    {
        query = query.field(
            Field::new(
                "staleDoogats",
                TypeRef::named_nn_list_nn("StaleDoogat"),
                |ctx| {
                    FieldFuture::new(async move {
                        let pool = ctx.data::<ReadPool>()?;
                        let type_filter = ctx
                            .args
                            .get("type")
                            .and_then(|v| v.string().ok())
                            .map(|s| s.to_string());
                        let stale = pool
                            .stale_doogats(type_filter)
                            .await
                            .map_err(to_server_error)?;
                        Ok(Some(FieldValue::list(stale.iter().map(|s| {
                            let mut obj = IndexMap::new();
                            obj.insert(Name::new("id"), GqlValue::from(s.id.as_str()));
                            obj.insert(Name::new("title"), GqlValue::from(s.title.as_str()));
                            obj.insert(
                                Name::new("doogatType"),
                                GqlValue::from(s.doogat_type.as_str()),
                            );
                            obj.insert(
                                Name::new("lastUpdated"),
                                GqlValue::from(s.last_updated.as_str()),
                            );
                            obj.insert(
                                Name::new("dateSource"),
                                GqlValue::from(s.date_source.to_string()),
                            );
                            obj.insert(Name::new("daysStale"), GqlValue::from(s.days_stale as i64));
                            obj.insert(
                                Name::new("thresholdDays"),
                                GqlValue::from(s.threshold_days as i64),
                            );
                            FieldValue::owned_any(GqlValue::Object(obj))
                        }))))
                    })
                },
            )
            .argument(InputValue::new("type", TypeRef::named(TypeRef::STRING)).description("Filter to a specific doogat type."))
            .description("Find typed doogats not updated within their type's staleness threshold."),
        );
    }

    // orphanDoogats(type: String): [OrphanDoogat!]!
    {
        query = query.field(
            Field::new(
                "orphanDoogats",
                TypeRef::named_nn_list_nn("OrphanDoogat"),
                |ctx| {
                    FieldFuture::new(async move {
                        let pool = ctx.data::<ReadPool>()?;
                        let type_filter = ctx
                            .args
                            .get("type")
                            .and_then(|v| v.string().ok())
                            .map(|s| s.to_string());
                        let orphans = pool
                            .orphan_doogats(type_filter)
                            .await
                            .map_err(to_server_error)?;
                        Ok(Some(FieldValue::list(orphans.iter().map(|o| {
                            let mut obj = IndexMap::new();
                            obj.insert(Name::new("id"), GqlValue::from(o.id.as_str()));
                            obj.insert(Name::new("title"), GqlValue::from(o.title.as_str()));
                            obj.insert(
                                Name::new("doogatType"),
                                GqlValue::from(o.doogat_type.as_str()),
                            );
                            obj.insert(
                                Name::new("outgoingLinks"),
                                GqlValue::from(o.outgoing_links as i64),
                            );
                            FieldValue::owned_any(GqlValue::Object(obj))
                        }))))
                    })
                },
            )
            .argument(InputValue::new("type", TypeRef::named(TypeRef::STRING)).description("Filter to a specific doogat type."))
            .description("Find typed doogats with no inbound links from other doogats."),
        );
    }

    // -- Sequence output types --
    let sequence_node_type = Object::new("SequenceNode")
        .description("A node in a sequence hierarchy (parent-child relationship).")
        .field(simple_field("id", TypeRef::named_nn(TypeRef::ID)))
        .field(simple_field("title", TypeRef::named_nn(TypeRef::STRING)));

    let sequence_info_type = Object::new("SequenceInfo")
        .description("Full sequence context for a doogat: parent, children, and breadcrumb.")
        .field(Field::new(
            "parent",
            TypeRef::named("SequenceNode"),
            |ctx| {
                FieldFuture::new(async move {
                    let info = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                    if let GqlValue::Object(obj) = info {
                        if let Some(p) = obj.get("parent") {
                            if matches!(p, GqlValue::Null) {
                                return Ok(None);
                            }
                            return Ok(Some(FieldValue::owned_any(p.clone())));
                        }
                    }
                    Ok(None)
                })
            },
        ))
        .field(Field::new(
            "children",
            TypeRef::named_nn_list_nn("SequenceNode"),
            |ctx| {
                FieldFuture::new(async move {
                    let info = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                    if let GqlValue::Object(obj) = info {
                        if let Some(GqlValue::List(children)) = obj.get("children") {
                            return Ok(Some(FieldValue::list(
                                children.iter().map(|c| FieldValue::owned_any(c.clone())),
                            )));
                        }
                    }
                    Ok(Some(FieldValue::list(std::iter::empty::<FieldValue>())))
                })
            },
        ))
        .field(Field::new(
            "breadcrumb",
            TypeRef::named_nn_list_nn("SequenceNode"),
            |ctx| {
                FieldFuture::new(async move {
                    let info = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                    if let GqlValue::Object(obj) = info {
                        if let Some(GqlValue::List(bc)) = obj.get("breadcrumb") {
                            return Ok(Some(FieldValue::list(
                                bc.iter().map(|c| FieldValue::owned_any(c.clone())),
                            )));
                        }
                    }
                    Ok(Some(FieldValue::list(std::iter::empty::<FieldValue>())))
                })
            },
        ));

    let broken_sequence_type = Object::new("BrokenSequence")
        .description("A doogat whose sequence parent reference points to a non-existent doogat.")
        .field(simple_field("doogatId", TypeRef::named_nn(TypeRef::ID)))
        .field(simple_field(
            "brokenParentId",
            TypeRef::named_nn(TypeRef::ID),
        ));

    fn seq_node_to_gql(n: &ddb_core::types::SequenceNode) -> GqlValue {
        let mut obj = IndexMap::new();
        obj.insert(Name::new("id"), GqlValue::from(n.id.as_str()));
        obj.insert(Name::new("title"), GqlValue::from(n.title.as_str()));
        GqlValue::Object(obj)
    }

    // sequenceInfo(id: ID!): SequenceInfo!
    {
        query = query.field(
            Field::new("sequenceInfo", TypeRef::named_nn("SequenceInfo"), |ctx| {
                FieldFuture::new(async move {
                    let pool = ctx.data::<ReadPool>()?;
                    let id = ctx.args.try_get("id")?.string()?.to_string();
                    let info = pool.sequence_info(id).await.map_err(to_server_error)?;

                    let parent_val = match &info.parent {
                        Some(p) => seq_node_to_gql(p),
                        None => GqlValue::Null,
                    };
                    let children_val =
                        GqlValue::List(info.children.iter().map(seq_node_to_gql).collect());
                    let bc_val =
                        GqlValue::List(info.breadcrumb.iter().map(seq_node_to_gql).collect());

                    let mut obj = IndexMap::new();
                    obj.insert(Name::new("parent"), parent_val);
                    obj.insert(Name::new("children"), children_val);
                    obj.insert(Name::new("breadcrumb"), bc_val);
                    Ok(Some(FieldValue::owned_any(GqlValue::Object(obj))))
                })
            })
            .argument(InputValue::new("id", TypeRef::named_nn(TypeRef::ID)).description("The doogat ID to get sequence info for."))
            .description("Get parent, children, and breadcrumb for a doogat in a sequence hierarchy."),
        );
    }

    // sequenceChildren(id: ID!): [SequenceNode!]!
    {
        query = query.field(
            Field::new(
                "sequenceChildren",
                TypeRef::named_nn_list_nn("SequenceNode"),
                |ctx| {
                    FieldFuture::new(async move {
                        let pool = ctx.data::<ReadPool>()?;
                        let id = ctx.args.try_get("id")?.string()?.to_string();
                        let children = pool.sequence_children(id).await.map_err(to_server_error)?;
                        Ok(Some(FieldValue::list(
                            children
                                .iter()
                                .map(|n| FieldValue::owned_any(seq_node_to_gql(n))),
                        )))
                    })
                },
            )
            .argument(InputValue::new("id", TypeRef::named_nn(TypeRef::ID)).description("The doogat ID to list children of."))
            .description("List direct children of a doogat in a sequence hierarchy."),
        );
    }

    // sequenceBreadcrumb(id: ID!): [SequenceNode!]!
    {
        query = query.field(
            Field::new(
                "sequenceBreadcrumb",
                TypeRef::named_nn_list_nn("SequenceNode"),
                |ctx| {
                    FieldFuture::new(async move {
                        let pool = ctx.data::<ReadPool>()?;
                        let id = ctx.args.try_get("id")?.string()?.to_string();
                        let bc = pool
                            .sequence_breadcrumb(id)
                            .await
                            .map_err(to_server_error)?;
                        Ok(Some(FieldValue::list(
                            bc.iter().map(|n| FieldValue::owned_any(seq_node_to_gql(n))),
                        )))
                    })
                },
            )
            .argument(InputValue::new("id", TypeRef::named_nn(TypeRef::ID)).description("The doogat ID to get breadcrumb for."))
            .description("Get ancestor chain from root to the given doogat in a sequence hierarchy."),
        );
    }

    // brokenSequences: [BrokenSequence!]!
    {
        query = query.field(Field::new(
            "brokenSequences",
            TypeRef::named_nn_list_nn("BrokenSequence"),
            |ctx| {
                FieldFuture::new(async move {
                    let pool = ctx.data::<ReadPool>()?;
                    let broken = pool.broken_sequences().await.map_err(to_server_error)?;
                    Ok(Some(FieldValue::list(broken.iter().map(|b| {
                        let mut obj = IndexMap::new();
                        obj.insert(Name::new("doogatId"), GqlValue::from(b.doogat_id.as_str()));
                        obj.insert(
                            Name::new("brokenParentId"),
                            GqlValue::from(b.broken_parent_id.as_str()),
                        );
                        FieldValue::owned_any(GqlValue::Object(obj))
                    }))))
                })
            },
        ).description("Find doogats whose sequence parent reference points to a non-existent doogat."));
    }

    DiscoveryOutput {
        query,
        sequence_node_type,
        sequence_info_type,
        broken_sequence_type,
    }
}
