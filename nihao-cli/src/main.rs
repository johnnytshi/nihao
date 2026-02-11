use clap::{Parser, Subcommand};
use nihao_core::{config::Config, FaceRecognizer};
use std::time::Instant;

#[derive(Parser)]
#[command(name = "nihao")]
#[command(about = "Facial authentication system for Linux", long_about = None)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Enable verbose logging
    #[arg(short, long, global = true)]
    verbose: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Enroll a new face
    Add {
        /// Username to enroll face for
        username: String,
        /// Optional label for this face
        #[arg(short, long)]
        label: Option<String>,
        /// Save debug visualization showing detected face
        #[arg(long)]
        debug: Option<String>,
    },
    /// Remove an enrolled face
    Remove {
        /// Username
        username: String,
        /// Face ID to remove
        face_id: String,
    },
    /// List enrolled faces
    List {
        /// Username to list faces for
        username: String,
    },
    /// Test face recognition
    Test {
        /// Username to test
        username: String,
        /// Show timing breakdown
        #[arg(short, long)]
        timing: bool,
    },
    /// Capture a snapshot from the camera
    Snapshot {
        /// Output file path
        output: String,
    },
    /// Show configuration
    Config {
        /// Validate configuration
        #[arg(long)]
        validate: bool,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Initialize logger
    let log_level = if cli.verbose { "debug" } else { "info" };
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(log_level))
        .init();

    match cli.command {
        Commands::Add { username, label, debug } => cmd_add(username, label, debug),
        Commands::Remove { username, face_id } => cmd_remove(username, face_id),
        Commands::List { username } => cmd_list(username),
        Commands::Test { username, timing } => cmd_test(username, timing),
        Commands::Snapshot { output } => cmd_snapshot(output),
        Commands::Config { validate } => cmd_config(validate),
    }
}

fn cmd_add(username: String, label: Option<String>, debug: Option<String>) -> anyhow::Result<()> {
    println!("Enrolling face for user: {}", username);
    if let Some(ref l) = label {
        println!("Label: {}", l);
    }

    let config = Config::load()?;
    let mut recognizer = FaceRecognizer::new(config.clone())?;

    println!("\nLook at the camera...");
    std::thread::sleep(std::time::Duration::from_secs(1));

    let face_id = recognizer.enroll_with_debug(&username, label, debug.as_deref())?;

    println!("\n✓ Face enrolled successfully!");
    println!("Face ID: {}", face_id);

    // Show where debug screenshot was saved
    if let Some(debug_path) = debug {
        println!("📷 Debug visualization saved to: {}", debug_path);
    } else if config.debug.save_screenshots {
        // Only show this message if user didn't provide explicit --debug path
        println!("📷 Debug screenshot saved to: {}/enroll_{}*.jpg",
                 config.debug.output_dir.display(), username);
    }

    Ok(())
}

fn cmd_remove(username: String, face_id: String) -> anyhow::Result<()> {
    println!("Removing face {} for user: {}", face_id, username);

    let config = Config::load()?;
    let mut recognizer = FaceRecognizer::new(config)?;

    recognizer.store_mut().remove_embedding(&username, &face_id)?;

    println!("✓ Face removed successfully");

    Ok(())
}

fn cmd_list(username: String) -> anyhow::Result<()> {
    let config = Config::load()?;
    let recognizer = FaceRecognizer::new(config)?;

    let faces = recognizer.store().list_faces(&username)?;

    if faces.is_empty() {
        println!("No faces enrolled for user: {}", username);
        return Ok(());
    }

    println!("Enrolled faces for {}:", username);
    println!();
    println!("{:<15} {:<20} {}", "Face ID", "Label", "Enrolled At");
    println!("{}", "-".repeat(60));

    for face in faces {
        let label = face.label.unwrap_or_else(|| "—".to_string());
        let enrolled_at = face.enrolled_at.format("%Y-%m-%d %H:%M:%S");
        println!("{:<15} {:<20} {}", face.id, label, enrolled_at);
    }

    Ok(())
}

fn cmd_test(username: String, show_timing: bool) -> anyhow::Result<()> {
    println!("Testing face recognition for user: {}", username);
    println!("\nLook at the camera...");

    let config = Config::load()?;
    let mut recognizer = FaceRecognizer::new(config.clone())?;

    let start = Instant::now();
    let result = recognizer.authenticate(&username)?;
    let duration = start.elapsed();

    println!();
    if result {
        println!("✅ Authentication successful!");

        // Show debug screenshot location
        if config.debug.save_screenshots {
            println!("📷 Debug screenshot saved to: {}/auth_{}*.jpg",
                     config.debug.output_dir.display(), username);
        }
    } else {
        println!("❌ Authentication failed: No match found");
    }

    if show_timing {
        println!("\nTiming:");
        println!("Total: {:.2}ms", duration.as_secs_f64() * 1000.0);
    } else {
        println!("Total time: {:.2}ms", duration.as_secs_f64() * 1000.0);
        println!("(Use --timing for detailed breakdown)");
    }

    Ok(())
}

fn cmd_snapshot(output: String) -> anyhow::Result<()> {
    println!("Capturing snapshot to: {}", output);

    let config = Config::load()?;
    let mut camera = nihao_core::capture::Camera::new(&config.camera)?;

    let frame = camera.capture_frame(false)?;  // No quality checks for snapshot
    frame.save(&output)?;

    println!("✓ Snapshot saved: {}", output);
    println!("Resolution: {}x{}", frame.width(), frame.height());

    Ok(())
}

fn cmd_config(validate: bool) -> anyhow::Result<()> {
    let config = Config::load()?;

    if validate {
        config.validate()?;
        println!("✓ Configuration is valid");
        return Ok(());
    }

    println!("Configuration:");
    println!();

    println!("[camera]");
    println!("  device = {:?}", config.camera.device);
    println!("  resolution = {}x{}", config.camera.width, config.camera.height);
    println!("  detection_scale = {}", config.camera.detection_scale);
    println!("  dark_threshold = {}", config.camera.dark_threshold);
    println!();

    println!("[detection]");
    println!("  model = {:?}", config.detection.model_path);
    println!(
        "  confidence_threshold = {}",
        config.detection.confidence_threshold
    );
    println!();

    println!("[embedding]");
    println!("  model = {:?}", config.embedding.model_path);
    println!();

    println!("[matching]");
    println!("  threshold = {}", config.matching.threshold);
    println!("  max_frames = {}", config.matching.max_frames);
    println!("  timeout = {}s", config.matching.timeout_secs);
    println!();

    println!("[runtime]");
    println!("  provider = CPU (GPU support removed)");
    println!();

    println!("[storage]");
    println!("  database_path = {:?}", config.storage.database_path);
    println!();

    println!("[debug]");
    println!("  save_screenshots = {}", config.debug.save_screenshots);
    println!("  output_dir = {:?}", config.debug.output_dir);

    Ok(())
}

