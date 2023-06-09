use actix_multipart::form::{tempfile::TempFile, MultipartForm};
use actix_web::{http::StatusCode, web, HttpResponse};
use cdn::{
	get_extension_from_filename, get_mime_type, get_redis_conn, CachedFile, Response, EXPIRE_TIME,
};
use redis::AsyncCommands;
use serde::Serialize;
use snowflake::SnowflakeIdGenerator;

use crate::{middlewares::auth::AuthorizationService, prisma};

#[derive(Debug, MultipartForm)]
pub struct UploadForm {
	// TODO: create a custom midleware to personalize the file limit
	#[multipart(rename = "file", limit = "10 MiB")]
	files: Vec<TempFile>,
}

#[derive(Debug, Serialize)]
pub struct FileData {
	id: i64,
	ext: String,
	path: String,
	size: i32,
}

#[actix_web::post("/file/upload")]
pub async fn route(
	_: AuthorizationService, // middleware
	MultipartForm(form): MultipartForm<UploadForm>,
	redis: web::Data<redis::Client>,
	client: web::Data<prisma::PrismaClient>,
) -> HttpResponse {
	let mut id_generator = SnowflakeIdGenerator::new(1, 1);
	let mut files: Vec<FileData> = Vec::new();
	let mut conn = get_redis_conn(redis.get_ref().clone()).await;

	for f in form.files {
		let file_name = f.file_name.clone().unwrap();
		let file_size = f.file.as_file().metadata().unwrap().len();
		let file_ext = get_extension_from_filename(file_name.as_str()).unwrap();
		let file_id = id_generator.real_time_generate();

		let upload_dir =
			std::env::var("UPLOADS_DIR").unwrap_or_else(|_| "/tmp/uploads".to_string());
		let temp_file_path = f.file.path();
		let path = format!("{}/{}.{}", upload_dir, file_id, file_ext);

		log::debug!("Moving file from {} to {}", path, temp_file_path.display());
		match std::fs::rename(temp_file_path, &path) {
			Ok(_) => {
				// cache path and content type in redis
				conn.set_ex::<String, CachedFile, ()>(
					file_id.to_string(),
					CachedFile {
						path: path.clone(),
						content_type: get_mime_type(path.clone()),
					},
					EXPIRE_TIME,
				)
				.await
				.unwrap();

				files.push(FileData {
					id: file_id,
					ext: file_ext.to_string(),
					path,
					size: file_size as i32,
				});
			}
			Err(_) => {
				HttpResponse::InternalServerError().finish();
			}
		}
	}

	let _ = client
		.file()
		.create_many(
			files
				.iter()
				.map(|file| {
					prisma::file::create_unchecked(
						file.id,
						file.path.clone(),
						file.size,
						get_mime_type(file.path.clone()),
						vec![],
					)
				})
				.collect(),
		)
		.exec()
		.await;

	HttpResponse::Created().json(Response::<Vec<FileData>> {
		status: StatusCode::CREATED.as_u16(),
		message: "Upload success",
		data: Some(files),
	})
}
