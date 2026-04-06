use async_graphql::dynamic::*;
use async_graphql::{Name, Value as GqlValue};
use futures_util::StreamExt;
use indexmap::IndexMap;

use std::collections::HashMap;

use crate::actor::ActorHandle;
use crate::events::EventKind;

use super::base_types::*;

/// Auxiliary types produced by the subscription builder that must be
/// registered on the schema alongside the Subscription object itself.
pub(crate) struct SubscriptionOutput {
    pub subscription: Subscription,
    pub change_event_type: Object,
}

pub(crate) fn build_subscription_fields(
    known_types: &HashMap<String, String>,
    type_schemas: &[ddb_core::types::TableSchema],
) -> SubscriptionOutput {
    // -- DoogatChangeEvent type --
    let change_event_type = Object::new("DoogatChangeEvent")
        .description("Real-time event emitted when a doogat is created, updated, or deleted.")
        .field(simple_field("action", TypeRef::named_nn(TypeRef::STRING)))
        .field(Field::new("doogat", TypeRef::named("Doogat"), |ctx| {
            FieldFuture::new(async move {
                let obj = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                Ok(obj_field(obj, "doogat"))
            })
        }))
        .field(simple_field("doogatId", TypeRef::named_nn(TypeRef::ID)));

    let mut subscription = Subscription::new("Subscription");

    // doogatChanged: DoogatChangeEvent! -- all events
    subscription = subscription.field(SubscriptionField::new(
        "doogatChanged",
        TypeRef::named_nn("DoogatChangeEvent"),
        |ctx| {
            let handle = ctx.data::<ActorHandle>().cloned();
            SubscriptionFieldFuture::new(async move {
                let handle = handle?;
                let event_bus = handle.event_bus().clone();
                let actor = handle;
                let rx = event_bus.subscribe();
                let stream = event_stream(rx).then(move |result| {
                    let actor = actor.clone();
                    async move {
                        let event = result?;
                        let action = match event.kind {
                            EventKind::Created => "created",
                            EventKind::Updated => "updated",
                            EventKind::Deleted => "deleted",
                        };
                        let doogat = if event.kind != EventKind::Deleted {
                            actor
                                .get_doogat(event.doogat_id.clone())
                                .await
                                .ok()
                                .map(|z| doogat_to_value(&z))
                        } else {
                            None
                        };
                        let mut map = IndexMap::new();
                        map.insert(Name::new("action"), GqlValue::from(action));
                        map.insert(
                            Name::new("doogatId"),
                            GqlValue::from(event.doogat_id.as_str()),
                        );
                        if let Some(z) = doogat {
                            map.insert(Name::new("doogat"), z);
                        }
                        Ok(FieldValue::owned_any(GqlValue::Object(map)))
                    }
                });
                Ok(stream)
            })
        },
    ));

    // doogatCreated: Doogat! -- only Created events
    subscription = subscription.field(SubscriptionField::new(
        "doogatCreated",
        TypeRef::named_nn("Doogat"),
        |ctx| {
            let handle = ctx.data::<ActorHandle>().cloned();
            SubscriptionFieldFuture::new(async move {
                let handle = handle?;
                let event_bus = handle.event_bus().clone();
                let actor = handle;
                let rx = event_bus.subscribe();
                let stream = event_stream(rx).filter_map(move |result| {
                    let actor = actor.clone();
                    async move {
                        let event = result.ok()?;
                        if event.kind != EventKind::Created {
                            return None;
                        }
                        let z = actor.get_doogat(event.doogat_id).await.ok()?;
                        Some(Ok(FieldValue::owned_any(doogat_to_value(&z))))
                    }
                });
                Ok(stream)
            })
        },
    ));

    // doogatUpdated: Doogat! -- only Updated events
    subscription = subscription.field(SubscriptionField::new(
        "doogatUpdated",
        TypeRef::named_nn("Doogat"),
        |ctx| {
            let handle = ctx.data::<ActorHandle>().cloned();
            SubscriptionFieldFuture::new(async move {
                let handle = handle?;
                let event_bus = handle.event_bus().clone();
                let actor = handle;
                let rx = event_bus.subscribe();
                let stream = event_stream(rx).filter_map(move |result| {
                    let actor = actor.clone();
                    async move {
                        let event = result.ok()?;
                        if event.kind != EventKind::Updated {
                            return None;
                        }
                        let z = actor.get_doogat(event.doogat_id).await.ok()?;
                        Some(Ok(FieldValue::owned_any(doogat_to_value(&z))))
                    }
                });
                Ok(stream)
            })
        },
    ));

    // doogatDeleted: ID! -- only Deleted events
    subscription = subscription.field(SubscriptionField::new(
        "doogatDeleted",
        TypeRef::named_nn(TypeRef::ID),
        |ctx| {
            let handle = ctx.data::<ActorHandle>().cloned();
            SubscriptionFieldFuture::new(async move {
                let handle = handle?;
                let event_bus = handle.event_bus().clone();
                let rx = event_bus.subscribe();
                let stream = event_stream(rx).filter_map(|result| async move {
                    let event = result.ok()?;
                    if event.kind != EventKind::Deleted {
                        return None;
                    }
                    Some(Ok(FieldValue::value(GqlValue::from(
                        event.doogat_id.as_str(),
                    ))))
                });
                Ok(stream)
            })
        },
    ));

    // Per-type subscription fields (e.g., contactChanged, bookmarkChanged)
    for schema in type_schemas {
        if !known_types.contains_key(&schema.table_name) {
            continue;
        }
        let field_name = format!("{}Changed", sanitize_field_name(&schema.table_name));
        let table_name = schema.table_name.clone();
        subscription = subscription.field(SubscriptionField::new(
            &field_name,
            TypeRef::named_nn("DoogatChangeEvent"),
            move |ctx| {
                let handle = ctx.data::<ActorHandle>().cloned();
                let table_name = table_name.clone();
                SubscriptionFieldFuture::new(async move {
                    let handle = handle?;
                    let event_bus = handle.event_bus().clone();
                    let actor = handle;
                    let rx = event_bus.subscribe();
                    let stream = event_stream(rx).filter_map(move |result| {
                        let actor = actor.clone();
                        let table_name = table_name.clone();
                        async move {
                            let event = result.ok()?;
                            if event.doogat_type.as_deref() != Some(&table_name) {
                                return None;
                            }
                            let action = match event.kind {
                                EventKind::Created => "created",
                                EventKind::Updated => "updated",
                                EventKind::Deleted => "deleted",
                            };
                            let doogat = if event.kind != EventKind::Deleted {
                                actor
                                    .get_doogat(event.doogat_id.clone())
                                    .await
                                    .ok()
                                    .map(|z| doogat_to_value(&z))
                            } else {
                                None
                            };
                            let mut map = IndexMap::new();
                            map.insert(Name::new("action"), GqlValue::from(action));
                            map.insert(
                                Name::new("doogatId"),
                                GqlValue::from(event.doogat_id.as_str()),
                            );
                            if let Some(z) = doogat {
                                map.insert(Name::new("doogat"), z);
                            }
                            Some(Ok(FieldValue::owned_any(GqlValue::Object(map))))
                        }
                    });
                    Ok(stream)
                })
            },
        ));
    }

    SubscriptionOutput {
        subscription,
        change_event_type,
    }
}
