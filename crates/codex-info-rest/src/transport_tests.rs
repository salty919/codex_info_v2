use super::*;
use codex_info_rest_contract::wire::sync::{Data, Receiver};
use std::fs;

#[test]
fn ui_receipt_suppresses_payload_and_subscription_wakes_only_on_change() {
    let mut server = RestServer::start_without_account(
        PublicState::Initializing,
        "127.0.0.1:0".parse().unwrap(),
    )
    .unwrap();
    let address = server.local_addr();
    let store = server.store();
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async{
  let socket=tokio::net::TcpStream::connect(address).await.unwrap();let(mut client,connection)=h2::client::handshake(socket).await.unwrap();let connection=tokio::spawn(async move{let _=connection.await;});
  let mut receiver=Receiver::default();let root=codex_info_rest_contract::wire::sync::hex(&receiver.root().unwrap());
  let request=http::Request::builder().uri(format!("http://{address}/state?parts=main")).header("codex-info-budget-ms","3000").header("codex-info-cache-root",&root).body(()).unwrap();
  let(response,_)=client.send_request(request,true).unwrap();let response=tokio::time::timeout(Duration::from_secs(3),response).await.unwrap().unwrap();let consumer=response.headers()["codex-info-consumer"].to_str().unwrap().to_owned();
  let mut body=response.into_body();let mut bytes=Vec::new();while let Some(chunk)=body.data().await{let chunk=chunk.unwrap();bytes.extend(&chunk);body.flow_control().release_capacity(chunk.len()).unwrap();}
  let first:Data=serde_json::from_slice(&bytes).unwrap();receiver.register_target(&codex_info_rest_contract::wire::request::Target::parse("/state?parts=main").unwrap().unwrap(),&first).unwrap();receiver.apply(&first).unwrap();let root=codex_info_rest_contract::wire::sync::hex(&receiver.root().unwrap());
  let request=http::Request::builder().method("POST").uri(format!("http://{address}/subscription")).header("codex-info-budget-ms","3000").header("codex-info-consumer",&consumer).header("codex-info-cache-root",&root).header("content-type","application/x-ndjson; charset=utf-8").body(()).unwrap();
  let(response,mut control)=client.send_request(request,false).unwrap();
  let open=json!({"type":"open","seq":1,"consumer":consumer,"cache_root":root,"max_definition_id":receiver.definitions.keys().max().unwrap(),"subscriptions":[{"view_id":null,"path":"/state","query":{"parts":"main"},"accepted_image":null}],"partial_receipts":[]});
  control.send_data(bytes::Bytes::from(format!("{open}\n")),false).unwrap();let response=tokio::time::timeout(Duration::from_secs(3),response).await.unwrap().unwrap();assert_eq!(response.status(),200);let mut notify=response.into_body();
  let ready=tokio::time::timeout(Duration::from_secs(3),notify.data()).await.unwrap().unwrap().unwrap();let value:Value=serde_json::from_slice(&ready).unwrap();assert_eq!(value["type"],"ready");assert_eq!(value["cache_state"],"matched");notify.flow_control().release_capacity(ready.len()).unwrap();
  let request=http::Request::builder().uri(format!("http://{address}/state?parts=main")).header("codex-info-budget-ms","3000").header("codex-info-consumer",&consumer).header("codex-info-cache-root",&root).body(()).unwrap();
  let(response,_)=client.send_request(request,true).unwrap();let response=tokio::time::timeout(Duration::from_secs(3),response).await.unwrap().unwrap();assert_eq!(response.status(),204,"committed content must never be transferred again");assert!(response.into_body().is_end_stream());
  let mut prepared=store.wire_prepared.subscribe();
  store.publish_account_boundary(PublicState::Initializing);
  tokio::time::timeout(Duration::from_secs(3),prepared.changed()).await.unwrap().unwrap();
  let check=json!({"type":"check","seq":2});control.send_data(bytes::Bytes::from(format!("{check}\n")),false).unwrap();
  let ack=tokio::time::timeout(Duration::from_secs(3),notify.data()).await.unwrap().unwrap().unwrap();let ack_value:Value=serde_json::from_slice(&ack).unwrap();assert_eq!(ack_value["type"],"ack");assert_eq!(ack_value["ack_seq"],2);notify.flow_control().release_capacity(ack.len()).unwrap();
  assert!(tokio::time::timeout(Duration::from_millis(100),notify.data()).await.is_err(),"same semantic publication must produce zero notifications after publisher barrier");
  store.publish_account_boundary(PublicState::AuthRequired);
  let changed=tokio::time::timeout(Duration::from_secs(3),notify.data()).await.unwrap().unwrap().unwrap();let changed:Value=serde_json::from_slice(&changed).unwrap();assert_eq!(changed["type"],"changed");assert_eq!(changed["views"].as_array().unwrap().len(),1);assert!(changed.get("definitions").is_none(),"notification has metadata only, not duplicate payload");
  assert_ne!(changed["views"][0]["target_image"],first.changes[0].target_image);
  let ack=json!({"type":"notify_ack","seq":3,"notify_seq":changed["notify_seq"]});control.send_data(bytes::Bytes::from(format!("{ack}\n")),false).unwrap();
  let ack=tokio::time::timeout(Duration::from_secs(3),notify.data()).await.unwrap().unwrap().unwrap();assert_eq!(serde_json::from_slice::<Value>(&ack).unwrap()["ack_seq"],3);notify.flow_control().release_capacity(ack.len()).unwrap();
  let request=http::Request::builder().uri(format!("http://{address}/state?parts=main")).header("codex-info-budget-ms","3000").header("codex-info-consumer",&consumer).header("codex-info-cache-root",&root).body(()).unwrap();
  let(response,_)=client.send_request(request,true).unwrap();let response=tokio::time::timeout(Duration::from_secs(3),response).await.unwrap().unwrap();assert_eq!(response.status(),200);let mut body=response.into_body();let mut bytes=Vec::new();while let Some(chunk)=body.data().await{let chunk=chunk.unwrap();bytes.extend(&chunk);body.flow_control().release_capacity(chunk.len()).unwrap();}
  let update:Data=serde_json::from_slice(&bytes).unwrap();assert_eq!(update.changes[0].target_image,changed["views"][0]["target_image"]);receiver.register_target(&codex_info_rest_contract::wire::request::Target::parse("/state?parts=main").unwrap().unwrap(),&update).unwrap();receiver.apply(&update).unwrap();
  let main=receiver.views[&update.changes[0].view_id].bindings.values().next().copied().unwrap();assert_eq!(receiver.resolve(main).unwrap()["acquisition_state"],"auth_required");
  let root=codex_info_rest_contract::wire::sync::hex(&receiver.root().unwrap());
  let receipt=json!({"type":"receipt","seq":4,"delivery_id":update.delivery_id,"complete_through":update.definitions.len()+update.patches.len()+update.fragments.len()+update.changes.len(),"partial":null,"cache_root":root});control.send_data(bytes::Bytes::from(format!("{receipt}\n")),false).unwrap();
  let ack=tokio::time::timeout(Duration::from_secs(3),notify.data()).await.unwrap().unwrap().unwrap();let ack_value:Value=serde_json::from_slice(&ack).unwrap();assert_eq!(ack_value["type"],"ack");assert_eq!(ack_value["ack_seq"],4);notify.flow_control().release_capacity(ack.len()).unwrap();
  let request=http::Request::builder().uri(format!("http://{address}/state?parts=main")).header("codex-info-budget-ms","3000").header("codex-info-consumer",&consumer).header("codex-info-cache-root",&root).body(()).unwrap();
  let(response,_)=client.send_request(request,true).unwrap();let response=tokio::time::timeout(Duration::from_secs(3),response).await.unwrap().unwrap();assert_eq!(response.status(),204);assert!(response.into_body().is_end_stream());
  control.send_reset(h2::Reason::CANCEL);connection.abort();
 });
    server.shutdown();
}

#[test]
fn publication_comparison_preserves_knownness_and_money_bits_without_version_projections() {
    assert!(!exact_fact(&Some(-0.0f64), &Some(0.0f64)));
    assert!(!exact_fact(&None::<f64>, &Some(0.0f64)));
    let first = PublishedSnapshot::account_boundary(
        PublicState::Initializing,
        1,
        "private-generation".into(),
    );
    let mut next = first.clone();
    next.generation = 2;
    next.pair = "different-transport-identity".into();
    assert!(same_public_domain(&first, &next));
    next.details.observed_at = Some(1800000000);
    assert!(!same_public_domain(&first, &next));
}

#[test]
fn thread_title_changes_do_not_change_main_projection() {
    transport::assert_confirmed_main_thread_dependency_boundaries();
}

#[test]
fn sixteen_consumers_keep_notify_and_seven_finite_streams_without_queue_starvation() {
    let mut server = RestServer::start_without_account(
        PublicState::Initializing,
        "127.0.0.1:0".parse().unwrap(),
    )
    .unwrap();
    let address = server.local_addr();
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async{
  let mut clients=Vec::new();let mut drivers=Vec::new();let mut controls=Vec::new();let mut notifications=Vec::new();
  for _ in 0..16{
   let socket=tokio::net::TcpStream::connect(address).await.unwrap();let(mut client,connection)=h2::client::handshake(socket).await.unwrap();drivers.push(tokio::spawn(async move{let _=connection.await;}));let mut receiver=Receiver::default();let empty=codex_info_rest_contract::wire::sync::hex(&receiver.root().unwrap());
   let request=http::Request::builder().uri(format!("http://{address}/state?parts=runtime")).header("codex-info-budget-ms","3000").header("codex-info-cache-root",&empty).body(()).unwrap();let(response,_)=client.send_request(request,true).unwrap();let response=tokio::time::timeout(Duration::from_secs(3),response).await.unwrap().unwrap();let consumer=response.headers()["codex-info-consumer"].to_str().unwrap().to_owned();let mut body=response.into_body();let mut bytes=Vec::new();while let Some(chunk)=body.data().await{let chunk=chunk.unwrap();bytes.extend(&chunk);body.flow_control().release_capacity(chunk.len()).unwrap();}let data:Data=serde_json::from_slice(&bytes).unwrap();let target=codex_info_rest_contract::wire::request::Target::parse("/state?parts=runtime").unwrap().unwrap();receiver.register_target(&target,&data).unwrap();receiver.apply(&data).unwrap();let root=codex_info_rest_contract::wire::sync::hex(&receiver.root().unwrap());
   let request=http::Request::builder().method("POST").uri(format!("http://{address}/subscription")).header("codex-info-budget-ms","3000").header("codex-info-consumer",&consumer).header("codex-info-cache-root",&root).header("content-type","application/x-ndjson; charset=utf-8").body(()).unwrap();let(response,mut control)=client.send_request(request,false).unwrap();let open=json!({"type":"open","seq":1,"consumer":consumer,"cache_root":root,"max_definition_id":receiver.definitions.keys().max(),"subscriptions":[{"view_id":null,"path":"/state","query":{"parts":"runtime"},"accepted_image":null}],"partial_receipts":[],"incoming_deliveries":[]});control.send_data(bytes::Bytes::from(format!("{open}\n")),false).unwrap();let response=tokio::time::timeout(Duration::from_secs(3),response).await.unwrap().unwrap();assert_eq!(response.status(),200);let mut notify=response.into_body();let ready=tokio::time::timeout(Duration::from_secs(3),notify.data()).await.unwrap().unwrap().unwrap();assert_eq!(serde_json::from_slice::<Value>(&ready).unwrap()["cache_state"],"matched");notify.flow_control().release_capacity(ready.len()).unwrap();controls.push(control);notifications.push(notify);clients.push((client,consumer,root));
  }
  let mut reads=tokio::task::JoinSet::new();for(client,consumer,root)in &mut clients{for _ in 0..7{let start=tokio::time::Instant::now();std::future::poll_fn(|cx|client.poll_ready(cx)).await.unwrap();let request=http::Request::builder().uri(format!("http://{address}/state?parts=runtime")).header("codex-info-budget-ms","3000").header("codex-info-consumer",&*consumer).header("codex-info-cache-root",&*root).body(()).unwrap();let(response,_)=client.send_request(request,true).unwrap();reads.spawn(async move{let response=tokio::time::timeout_at(start+Duration::from_secs(3),response).await.unwrap().unwrap();assert_eq!(response.status(),204);assert!(response.into_body().is_end_stream());assert!(start.elapsed()<Duration::from_secs(1));});}}
  let mut count=0;while let Some(result)=reads.join_next().await{result.unwrap();count+=1;}assert_eq!(count,112);assert_eq!(controls.len(),16);assert_eq!(notifications.len(),16);for control in &mut controls{control.send_reset(h2::Reason::CANCEL);}for driver in drivers{driver.abort();}
 });
    server.shutdown();
}

#[test]
fn versionless_failures_keep_fixed_diagnostics_without_private_request_data() {
    let directory = tempfile::tempdir().unwrap();
    let mut server = RestServer::start_without_account(
        PublicState::Initializing,
        "127.0.0.1:0".parse().unwrap(),
    )
    .unwrap();
    server.store().diagnostics.set_data_root(directory.path());
    let address = server.local_addr();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let socket = tokio::net::TcpStream::connect(address).await.unwrap();
            let (mut client, driver) = h2::client::handshake(socket).await.unwrap();
            let driver = tokio::spawn(async move {
                let _ = driver.await;
            });
            let request = http::Request::builder()
                .method("POST")
                .uri(format!(
                    "http://{address}/state?parts=main&account=account-987"
                ))
                .header("codex-info-budget-ms", "3000")
                .header("authorization", "Bearer never-record-fixture-secret")
                .body(())
                .unwrap();
            let (response, _) = client.send_request(request, true).unwrap();
            let response = tokio::time::timeout(Duration::from_secs(3), response)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(response.status(), 405);
            driver.abort();
        });
    let until = std::time::Instant::now() + Duration::from_secs(3);
    let text = loop {
        let text = fs::read_dir(directory.path().join("logs/rest"))
            .ok()
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter_map(|entry| fs::read_to_string(entry.path()).ok())
            .collect::<String>();
        if !text.is_empty() {
            break text;
        }
        assert!(
            std::time::Instant::now() < until,
            "failure diagnostic must reach its existing owner"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    let entries = text
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert!(entries.iter().any(|entry| entry["route"] == "/state"
        && entry["stage"] == "request_parse"
        && entry["reason"] == "bad_request"
        && entry["http_status"] == 405));
    assert!(!text.contains("account-987"));
    assert!(!text.contains("never-record"));
    assert!(!text.contains("authorization"));
    server.shutdown();
}
