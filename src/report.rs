use std::fs::{create_dir_all, File};
use std::path::Path;

use crate::time::time_now;
use crate::State;
use crate::{metrics::MetricsReport, time::execution_id};

#[derive(serde::Serialize, Default, Debug)]
struct Report {
    execution_id: String,
    homeserver: String,
    step: usize,
    step_users: usize,
    users_to_act: usize,
    step_friendships: usize,
    report: MetricsReport,
}

pub struct ReportManager {
    output_dir: String,
    pub execution_id: String,
}

impl ReportManager {
    pub fn with_output_dir(output_dir: String) -> Self {
        let execution_id = Self::compute_execution_id(&output_dir);
        Self::ensure_execution_directory(&output_dir, &execution_id);
        Self {
            output_dir,
            execution_id,
        }
    }

    ///
    /// Generates a unique and human-readable execution id based on the local date and an optional
    /// sequence number if the original directory already exists.
    ///
    fn compute_execution_id(output_dir: &str) -> String {
        let original_execution_id = execution_id();

        let mut execution_id = original_execution_id.clone();
        let mut reports_dir = Self::compute_reports_dir(output_dir, &execution_id);
        let mut path = Path::new(&reports_dir);
        let mut i = 0;

        while path.exists() {
            i += 1;
            execution_id = format!("{}_{}", original_execution_id, i);
            reports_dir = Self::compute_reports_dir(output_dir, &execution_id);
            path = Path::new(&reports_dir);
        }

        execution_id
    }

    fn compute_reports_dir(output_dir: &str, execution_id: &str) -> String {
        format!("{}/{}", output_dir, execution_id)
    }

    pub fn generate_report(
        &self,
        state: &State,
        users_to_act: usize,
        step: usize,
        report: MetricsReport,
    ) {
        let reports_dir = Self::compute_reports_dir(&self.output_dir, &self.execution_id);

        let path = format!("{}/report_{}_{}.yaml", reports_dir, step, time_now());
        let buffer = File::create(&path).unwrap();

        let report = Report {
            execution_id: self.execution_id.to_owned(),
            homeserver: state.config.homeserver_url.to_string(),
            step,
            users_to_act,
            step_users: state.users.len(),
            step_friendships: state.friendships.len(),
            report,
        };

        serde_yaml::to_writer(buffer, &report).expect("Couldn't write report to file");
        println!("Step report generated: {}\n", path);
        println!("{:#?}\n", report);

        // print new line in between steps
        if step < state.config.total_steps {
            println!();
        }
    }

    ///
    /// Ensures the existence of the output and execution directories and the capacity of the tool
    /// to create files and write to both.
    ///
    /// # Panics
    ///
    /// If we are not able to create the directory for the current execution.
    ///
    fn ensure_execution_directory(output_dir: &str, execution_id: &str) {
        let directory = Self::compute_reports_dir(output_dir, execution_id);

        create_dir_all(directory.clone())
            .unwrap_or_else(|_| panic!("could not create output directory {}", directory));
    }
}
