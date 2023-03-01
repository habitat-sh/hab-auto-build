use color_eyre::eyre::Result;
use lazy_static::lazy_static;
use reqwest::{blocking::ClientBuilder, header, redirect::Policy, Method, Url};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};
use tracing::debug;

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

    fn calculate_ranges(content_length: u64) -> Vec<(String, u64, u64, u64)> {
        let mut range_start = 0;
        let mut ranges = vec![];
        let chunk_size = content_length / *DOWNLOAD_THREAD_COUNT - 1;

        for thread in 0..*DOWNLOAD_THREAD_COUNT {
            let mut range_end = chunk_size + range_start;
            if thread == (*DOWNLOAD_THREAD_COUNT - 1) {
                range_end = content_length
            }

            let range: String = format!("bytes={}-{}", range_start, range_end);
            let range_to_process: u64 = range_end - range_start;
            let buffer_chunks: u64 = range_to_process / *DOWNLOAD_MEMORY_BUFFER;
            let chunk_remainder: u64 = range_to_process % *DOWNLOAD_MEMORY_BUFFER;

            ranges.push((range, range_start, buffer_chunks, chunk_remainder));
            range_start = range_start + chunk_size + 1;
        }
        ranges
    }

    pub fn execute(self) -> Result<()> {
        let client = ClientBuilder::new().redirect(Policy::none()).build()?;

        let mut url = self.url.clone();
        let mut final_response = None;
        let mut file_content_length = None;
        let mut base_headers = reqwest::header::HeaderMap::new();
        // We put a common user agent as some remote hosts forbid downloads otherwise
        base_headers.append(header::USER_AGENT, "curl/7.68.0".parse().unwrap());
        base_headers.append(header::RANGE, "".parse().unwrap());

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

            let response = client.execute(request)?;
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
                file_content_length = response.content_length();
                final_response = Some(response);
            }
        }

        match file_content_length {
            Some(file_content_length) => {
                debug!("Starting mult-threaded download of file from {}", url);

                let ranges = Download::calculate_ranges(file_content_length);

                std::thread::scope(|scope| {
                    let mut children = vec![];
                    for (range, range_start, buffer_chunks, chunk_remainder) in ranges {
                        children.push(scope.spawn({
                            let filename = self.filename.as_path();
                            let client = &client;
                            let url = url.as_ref();
                            move || {
                                let mut file =
                                    File::create(filename).expect("Failed to create file");
                                file.seek(SeekFrom::Start(range_start))
                                    .expect("Failed to seek range in file");

                                let mut file_range_res = client
                                    .get(url)
                                    .header(header::RANGE, range)
                                    .send()
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

                Ok(())
            }
            None => {
                debug!("Starting single-threaded download of file from {}", url);
                let response = client.get(url.as_ref()).send()?;
                let mut file = File::create(self.filename)?;
                file.write_all(&response.bytes()?)?;
                file.sync_all()?;
                Ok(())
            }
        }
    }
}
