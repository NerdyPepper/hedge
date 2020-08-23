use anyhow::{Context, Result};
use hyper::header::CONTENT_TYPE;
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Method, Request, Response, Server, StatusCode};
use multer::Multipart;
use nanoid::nanoid;
use rusqlite::{params, Connection, OpenFlags, NO_PARAMS};
use url::form_urlencoded;

use std::collections::HashMap;
use std::path::Path;

fn respond_with_shortlink<S: AsRef<str>>(shortlink: S) -> Response<Body> {
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/html")
        .body(Body::from(shortlink.as_ref().to_string()))
        .unwrap()
}

fn respond_with_status(s: StatusCode) -> Response<Body> {
    Response::builder().status(s).body(Body::empty()).unwrap()
}

fn shorten<S: AsRef<str>>(url: S, conn: &mut Connection) -> Result<String> {
    let mut stmt = conn.prepare("select * from urls where link = ?1")?;
    let mut rows = stmt.query(params![url.as_ref().to_string()])?;
    if let Some(row) = rows.next()? {
        return Ok(row.get(1)?);
    } else {
        let new_id = nanoid!(4);
        conn.execute(
            "insert into urls (link, shortlink) values (?1, ?2)",
            params![url.as_ref().to_string(), new_id],
        )?;
        return Ok(new_id);
    }
}

fn get_link<S: AsRef<str>>(url: S, conn: &mut Connection) -> Result<Option<String>> {
    let url = url.as_ref();
    let mut stmt = conn.prepare("select * from urls where shortlink = ?1")?;
    let mut rows = stmt.query(params![url.to_string()])?;
    if let Some(row) = rows.next()? {
        return Ok(row.get(0)?);
    } else {
        return Ok(None);
    }
}

async fn process_multipart(
    body: Body,
    boundary: String,
    conn: &mut Connection,
) -> Result<Response<Body>> {
    let mut m = Multipart::new(body, boundary);
    if let Some(field) = m.next_field().await? {
        if field.name() == Some("shorten") {
            let content = field
                .text()
                .await
                .with_context(|| format!("Expected field name"))?;

            let shortlink = shorten(content, conn)?;
            return Ok(respond_with_shortlink(shortlink));
        }
    }
    Ok(Response::builder()
        .status(StatusCode::OK)
        .body(Body::empty())?)
}

async fn shortner_service(req: Request<Body>) -> Result<Response<Body>> {
    let mut conn = init_db("./urls.db_3").unwrap();

    match req.method() {
        &Method::POST => {
            let boundary = req
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|ct| ct.to_str().ok())
                .and_then(|ct| multer::parse_boundary(ct).ok());

            if boundary.is_none() {
                let b = hyper::body::to_bytes(req)
                    .await
                    .with_context(|| format!("Failed to stream request body!"))?;

                let params = form_urlencoded::parse(b.as_ref())
                    .into_owned()
                    .collect::<HashMap<String, String>>();

                if let Some(n) = params.get("shorten") {
                    let s = shorten(n, &mut conn)?;
                    return Ok(respond_with_shortlink(s));
                } else {
                    return Ok(respond_with_status(StatusCode::UNPROCESSABLE_ENTITY));
                }
            }

            return process_multipart(req.into_body(), boundary.unwrap(), &mut conn).await;
        }
        &Method::GET => {
            let shortlink = req.uri().path().to_string();
            let link = get_link(&shortlink[1..], &mut conn);
            if let Some(l) = link.unwrap() {
                Ok(Response::builder()
                    .header("Location", &l)
                    .header("content-type", "text/html")
                    .status(StatusCode::MOVED_PERMANENTLY)
                    .body(Body::from(format!(
                        "You will be redirected to: {}. If not, click the link.",
                        &l
                    )))?)
            } else {
                Ok(Response::builder()
                    .status(StatusCode::NOT_FOUND)
                    .body(Body::empty())?)
            }
        }
        _ => {
            return Ok(Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Body::empty())?)
        }
    }
}

fn init_db<P: AsRef<Path>>(p: P) -> Result<Connection> {
    let conn = Connection::open_with_flags(
        p,
        OpenFlags::SQLITE_OPEN_CREATE | OpenFlags::SQLITE_OPEN_READ_WRITE,
    )?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS urls (
            link TEXT PRIMARY KEY,
            shortlink TEXT NOT NULL
        )",
        NO_PARAMS,
    )?;
    return Ok(conn);
}

fn main() -> Result<()> {
    smol::run(async {
        let addr = ([127, 0, 0, 1], 3000).into();
        let service =
            make_service_fn(|_| async { Ok::<_, hyper::Error>(service_fn(shortner_service)) });
        let server = Server::bind(&addr).serve(service);
        println!("Listening on http://{}", addr);
        server.await.unwrap();
        Ok(())
    })
}
