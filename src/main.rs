use std::env;

use codegen::{Scope, Type};
use inflector::cases::classcase::to_class_case;
use itertools::Itertools;
use sqlx::PgPool;

#[tokio::main]
async fn main() {
    do_it().await.unwrap();
}

async fn do_it() -> Result<(), anyhow::Error> {
    let db_url = env::var("DATABASE_URL")?;
    let pool = PgPool::connect(&db_url).await?;

    let tables = sqlx::query!(
        "SELECT table_name, column_name, data_type, is_nullable FROM information_schema.columns WHERE table_schema='public' ORDER BY table_name, ordinal_position"
    )
    .fetch_all(&pool)
    .await?;
    let grouped = tables
        .into_iter()
        .group_by(|t| t.table_name.clone().unwrap());
    let mut scope = Scope::new();
    scope.import("sqlx", "PgPool");
    for (table_name, columns) in &grouped {
        //println!("{}", table.table_name.unwrap());
        if !should_emit(&table_name) {
            continue;
        }

        let columns_vec: Vec<_> = columns
            .map(|c| {
                (
                    c.column_name.unwrap(),
                    c.data_type.unwrap(),
                    c.is_nullable.unwrap(),
                )
            })
            .collect();
        add_structs_for_table(&mut scope, &table_name, &columns_vec);
        add_insert_for_table(&mut scope, &table_name, &columns_vec);
        // add_select_for_table(&mut scope, &table_name, &columns_vec);
    }
    println!("{}", scope.to_string());
    Ok(())
}

fn should_emit(table_name: &str) -> bool {
    table_name != "_sqlx_migrations"
}

fn add_insert_for_table(scope: &mut Scope, table_name: &str, columns: &[(String, String, String)]) {
    let new_fn = scope.new_fn(&format!("insert_{}", table_name));
    new_fn.set_async(true);
    new_fn.vis("pub");
    new_fn.arg("conn", Type::new("&PgPool"));
    new_fn.arg(
        "row",
        Type::new(&format!("&{}", input_row_struct_name(table_name))),
    );
    new_fn.ret(Type::new(&format!(
        "Result<{}, sqlx::Error>",
        row_struct_name(table_name)
    )));
    let columns: Vec<_> = columns
        .iter()
        .filter(|c| c.0 != "id" && c.0 != "created_at")
        .collect();
    let insert_name_list = columns.iter().map(|c| format!("\"{}\"", c.0)).join(", ");
    let args_list = columns.iter().map(|c| format!("row.{}", c.0)).join(", ");
    let insert_placeholders = columns
        .iter()
        .enumerate()
        .map(|(i, _)| format!("${}", (i + 1)))
        .join(", ");

    let body = format!(
        r##"
    let result = sqlx::query_as!({},
        r#"INSERT INTO {} ({})
        VALUES ({})
        RETURNING *"#,
        {}
    )
    .fetch_one(conn)
    .await?;
    Ok(result)"##,
        row_struct_name(table_name),
        table_name,
        insert_name_list,
        insert_placeholders,
        args_list
    );
    new_fn.line(body);
}

fn add_select_for_table(scope: &mut Scope, table_name: &str, columns: &[(String, String, String)]) {
    let Some(id_typ) = &columns.iter().find(|c| c.0 == "id") else {
        return
    };
    let new_fn = scope.new_fn(&format!("select_{}", table_name));
    new_fn.set_async(true);
    new_fn.vis("pub");
    new_fn.arg("conn", Type::new("&mut PgConnection"));
    new_fn.arg(
        "id",
        Type::new(&format!("&{}", pg_type_to_rs_type(&id_typ.1))),
    );
    new_fn.ret(Type::new(&format!(
        "Result<{}, sqlx::Error>",
        row_struct_name(table_name)
    )));
    let insert_name_list = columns.iter().map(|c| format!("\"{}\"", c.0)).join(", ");

    let body = format!(
        r##"
    let result = sqlx::query_as!({},
        r#"SELECT {} FROM {} WHERE id=$1"#, id
    )
    .fetch_one(&mut *conn)
    .await?;
    Ok(result)"##,
        row_struct_name(table_name),
        insert_name_list,
        table_name
    );
    new_fn.line(body);
}

fn input_row_struct_name(table_name: &str) -> String {
    format!("{}InputRow", to_class_case(table_name))
}

fn row_struct_name(table_name: &str) -> String {
    format!("{}Row", to_class_case(table_name))
}

fn add_structs_for_table(
    scope: &mut Scope,
    table_name: &str,
    columns: &[(String, String, String)],
) {
    let columns = columns
        .iter()
        .map(|(name, ty, is_option)| {
            let ty = pg_type_to_rs_type(ty);
            let ty = if is_option == "YES" {
                format!("Option<{}>", ty)
            } else {
                ty
            };
            (name, ty)
        })
        .collect_vec();
    let new_struct = scope.new_struct(&row_struct_name(table_name));
    new_struct.derive("Debug");
    new_struct.derive("Clone");
    new_struct.vis("pub");
    for column in &columns {
        new_struct.field(&format!("pub {}", column.0), &column.1);
    }
    let new_in_struct = scope.new_struct(&input_row_struct_name(table_name));
    new_in_struct.vis("pub");
    for column in columns {
        if column.0 != "id" && column.0 != "created_at" {
            new_in_struct.field(&format!("pub {}", column.0), &column.1);
        }
    }
}

fn pg_type_to_rs_type(pg_type: &str) -> String {
    match pg_type {
        "integer" => "i32",
        "bigint" => "i64",
        "real" => "f32",
        "text" => "String",
        "character varying" => "String",
        "timestamp with time zone" => "chrono::DateTime<chrono::Utc>",
        "boolean" => "bool",
        "bytea" => "Vec<u8>", // is this right?
        "USER-DEFINED" => "()",
        "numeric" => "bigdecimal::BigDecimal",
        _ => panic!("Unknown type: {}", pg_type),
    }
    .to_string()
}
