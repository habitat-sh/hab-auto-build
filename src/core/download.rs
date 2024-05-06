use color_eyre::eyre::{eyre, Result};
use lazy_static::lazy_static;
use reqwest::{
    blocking::{Client, ClientBuilder, Request, Response},
    header,
    redirect::Policy,
    Method, StatusCode, Url,
};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::Instant,
};
use suppaftp::FtpStream;
use tracing::{debug, log::error};

lazy_static! {
    static ref DOWNLOAD_THREAD_COUNT: u64 = num_cpus::get() as u64;
    static ref DOWNLOAD_MEMORY_BUFFER: u64 = 1024 * 256;
}

pub struct Download {
    pub url: Url,
    pub filename: PathBuf,
}

impl Download {
    pub fn new(url: &Url, filename: impl AsRef<Path>) -> Download {
        Download {
            url: url.clone(),
            filename: filename.as_ref().to_path_buf(),
        }
    }

    fn calculate_ranges(content_length: u64) -> Vec<(u64, u64, u64, u64)> {
        let mut range_start = 0;
        let mut ranges = vec![];
        let chunk_size = content_length / *DOWNLOAD_THREAD_COUNT - 1;

        for thread in 0..*DOWNLOAD_THREAD_COUNT {
            let mut range_end = chunk_size + range_start;
            if thread == (*DOWNLOAD_THREAD_COUNT - 1) {
                range_end = content_length
            }

            let range_to_process: u64 = range_end - range_start;
            let buffer_chunks: u64 = range_to_process / *DOWNLOAD_MEMORY_BUFFER;
            let chunk_remainder: u64 = range_to_process % *DOWNLOAD_MEMORY_BUFFER;

            ranges.push((range_start, range_end, buffer_chunks, chunk_remainder));
            range_start = range_start + chunk_size + 1;
        }
        ranges
    }

    pub fn execute(self) -> Result<()> {
        match self.url.scheme() {
            "http" | "https" => self.execute_http(),
            "ftp" => self.execute_ftp(),
            _ => Err(eyre!("Unsupported download protocol")),
        }
    }
    fn execute_ftp(self) -> Result<()> {
        let host = &self
            .url
            .host_str()
            .ok_or(eyre!("URL '{}' does not specify a host", &self.url))?;
        let port = self.url.port().unwrap_or(21);
        let remote_file = PathBuf::from(self.url.path());
        let remote_file_parent_dir = remote_file.parent().and_then(|v| v.to_str());
        let remote_file_name = remote_file
            .file_name()
            .and_then(|v| v.to_str())
            .ok_or(eyre!("URL '{}' missing file name", &self.url))?;
        debug!("Connecting to FTP server {}:{}", host, port);
        let mut ftp_stream = FtpStream::connect(format!("{}:{}", host, port))?;
        let (username, password) = match (self.url.username(), self.url.password()) {
            ("", _) => ("anonymous", format!("anonymous@{}", host)),
            (username, Some(password)) => (username, password.to_owned()),
            (username, None) => (username, format!("{}@{}", username, host)),
        };
        debug!("Logging into {}:{} as {}", host, port, username);
        ftp_stream.login(username, password.as_str())?;
        ftp_stream.transfer_type(suppaftp::types::FileType::Binary)?;
        if let Some(parent_dir) = remote_file_parent_dir {
            debug!("Changing to directory {}", parent_dir);
            ftp_stream.cwd(parent_dir)?;
        }
        debug!("Retrieving file {}", remote_file_name);
        let file_size = ftp_stream.size(self.url.path())? as u64;
        debug!(
            "Retrieved size for {}: {} bytes",
            remote_file_name, file_size
        );
        let mut stream = ftp_stream.retr_as_stream(remote_file_name)?;
        let mut file = File::create(self.filename.as_path())?;
        let buffer_chunks: u64 = file_size / *DOWNLOAD_MEMORY_BUFFER;
        let chunk_remainder: u64 = file_size % *DOWNLOAD_MEMORY_BUFFER;
        for _ in 0..buffer_chunks {
            let mut buffer = vec![0u8; *DOWNLOAD_MEMORY_BUFFER as usize];
            stream
                .read_exact(&mut buffer)
                .expect("Failed to read ftp stream into buffer");
            file.write_all(&buffer)
                .expect("Failed to write buffered data to file");
        }
        if chunk_remainder != 0 {
            let mut buffer = Vec::with_capacity(*DOWNLOAD_MEMORY_BUFFER as usize);
            let bytes_read = stream
                .read_to_end(&mut buffer)
                .expect("Failed to read ftp stream into buffer");
            file.write_all(&buffer[..bytes_read])
                .expect("Failed to write buffered data to file");
        }
        file.sync_all().expect("Failed to sync file data");

        Ok(())
    }

    fn execute_http(self) -> Result<()> {
        let client = ClientBuilder::new()
            .redirect(Policy::none())
            .no_gzip()
            .no_deflate()
            .no_brotli()
            .tcp_nodelay(true)
            .build()?;

        let mut url = self.url.clone();
        let mut final_response = None;
        let mut base_headers = reqwest::header::HeaderMap::new();
        // We put a common user agent as some remote hosts forbid downloads otherwise
        base_headers.append(header::USER_AGENT, "curl/7.81.0".parse().unwrap());
        let mut additional_headers = reqwest::header::HeaderMap::new();
        while final_response.is_none() {
            let mut request = reqwest::blocking::Request::new(Method::GET, url.clone());
            request
                .headers_mut()
                .extend(base_headers.clone().into_iter());
            request
                .headers_mut()
                .extend(additional_headers.clone().into_iter());
            additional_headers.clear();

            let response = Download::execute_request(&client, request)?;
            let headers = response.headers();
            if let Some(content_disposition) = headers.get(header::CONTENT_DISPOSITION) {
                if let Ok(value) = content_disposition.to_str() {
                    debug!("Received content disposition header {}", value);
                    if value.trim().starts_with("attachment") {
                        additional_headers
                            .append(header::CONTENT_DISPOSITION, content_disposition.to_owned());
                    }
                }
            }
            if let Some(redirect_url) = headers.get(header::LOCATION) {
                debug!("Redirecting to {}", redirect_url.to_str()?);
                url = Url::parse(redirect_url.to_str()?)?;
            } else {
                final_response = Some(response);
            }
        }

        // Test if the server responds correctly to range requests
        let mut request = reqwest::blocking::Request::new(Method::GET, url.clone());
        request.headers_mut().extend(base_headers.clone());
        request
            .headers_mut()
            .insert(header::RANGE, "bytes=0-0".parse().unwrap());
        let response = Download::execute_request(&client, request)?;
        let file_content_length = if let (StatusCode::PARTIAL_CONTENT, Some(Ok(range))) = (
            response.status(),
            response
                .headers()
                .get(header::CONTENT_RANGE)
                .map(|v| v.to_str()),
        ) {
            if let Some(Ok(value)) = range
                .trim()
                .strip_prefix("bytes 0-0/")
                .map(|v| v.parse::<u64>())
            {
                Some(value)
            } else {
                None
            }
        } else {
            None
        };

        match file_content_length {
            Some(file_content_length) => {
                let start = Instant::now();
                debug!("Starting multi-threaded download of file from {}", url);
                let ranges = Download::calculate_ranges(file_content_length);
                std::thread::scope(|scope| {
                    let mut children = vec![];
                    for (range_start, range_end, buffer_chunks, chunk_remainder) in ranges {
                        children.push(scope.spawn({
                            let filename = self.filename.as_path();
                            let client = &client;
                            let base_headers = base_headers.clone();
                            let url = url.clone();
                            move || {
                                let mut file =
                                    File::create(filename).expect("Failed to create file");
                                file.seek(SeekFrom::Start(range_start))
                                    .expect("Failed to seek range in file");

                                let mut request = reqwest::blocking::Request::new(Method::GET, url);
                                request.headers_mut().extend(base_headers.into_iter());
                                request.headers_mut().insert(
                                    header::RANGE,
                                    format!("bytes={}-{}", range_start, range_end)
                                        .parse()
                                        .unwrap(),
                                );
                                let mut file_range_res = Download::execute_request(client, request)
                                    .expect("Failed to send request to download file");
                                for _ in 0..buffer_chunks {
                                    let mut buffer = vec![0u8; *DOWNLOAD_MEMORY_BUFFER as usize];
                                    let range = file_range_res.by_ref();
                                    range
                                        .read_exact(&mut buffer)
                                        .expect("Failed to read reponse data into buffer");
                                    file.write_all(&buffer)
                                        .expect("Failed to write buffered data to file");
                                }
                                file.sync_all().expect("Failed to sync file data");

                                if chunk_remainder != 0 {
                                    file_range_res
                                        .copy_to(&mut file)
                                        .expect("Failed to copy remaining reponse data to file");
                                }
                            }
                        }));
                    }

                    for child in children {
                        let _ = child.join();
                    }
                });
                debug!(
                    "Finished multi-threaded download of file from {} in {}s",
                    url,
                    start.elapsed().as_secs_f32()
                );
                Ok(())
            }
            None => {
                let start = Instant::now();
                debug!("Starting single-threaded download of file from {}", url);
                let mut request = reqwest::blocking::Request::new(Method::GET, url.clone());
                request.headers_mut().extend(base_headers);
                let response = Download::execute_request(&client, request)?;
                let mut file = File::create(self.filename)?;
                file.write_all(&response.bytes()?)?;
                file.sync_all()?;
                debug!(
                    "Finished single-threaded download of file from {} in {}s",
                    url,
                    start.elapsed().as_secs_f32()
                );
                Ok(())
            }
        }
    }

    fn execute_request(client: &Client, request: Request) -> reqwest::Result<Response> {
        let mut attempts_remaining = 10;
        loop {
            let cloned_request = request.try_clone().unwrap();
            match client.execute(cloned_request) {
                Ok(response) => return Ok(response),
                Err(err) => {
                    error!("Failed to complete HTTP request: {}", err);
                    attempts_remaining -= 1;
                    if attempts_remaining == 0 {
                        return Err(err);
                    }
                }
            }
        }
    }
}
