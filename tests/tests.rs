use ch_grafana_cache::clickhouse;

#[tokio::test]
async fn clickhouse() -> anyhow::Result<()> {
    let ch = clickhouse::ChClient::from_flags(&clickhouse::Flags {
        url: "http://localhost:8123".parse()?,
        username: "default".into(),
        password: None,
    });

    let bytes = ch
        .query_native("SELECT * from system.zeros LIMIT 0".into())
        .await?;
    assert_eq!(bytes, 0);

    let query = "SELECT number, number + 1 FROM system.numbers LIMIT 3";

    let bytes = ch.query_native(query.into()).await?;
    assert_eq!(bytes, 87);

    let r = ch.query(query.into(), false).await?;
    assert_eq!(
        r,
        (0..3)
            .map(|i| clickhouse::ResultRow {
                cols: vec![i.to_string(), (i + 1).to_string()]
            })
            .collect::<Vec<_>>()
    );

    let r = ch.send_query(query.into(), "CSV").await?.text().await?;
    assert_eq!("0,1\n1,2\n2,3\n", r);
    Ok(())
}
