extern crate core;

use clap::Parser as Clap;
use jagua_rs::io::import::Importer;
use log::{info, warn, Level};
use rand::SeedableRng;
use sparrow::config::*;
use sparrow::optimizer::optimize;
use sparrow::util::io;
use sparrow::util::io::{ExtSPOutput, MainCli};
use sparrow::EPOCH;
use std::fs;
use std::path::Path;
use std::time::Duration;
use anyhow::{bail, Result};
use jagua_rs::probs::spp::entities::{SPProblem, SPSolution};
use jagua_rs::entities::{Layout,PItemKey,PlacedItem};
use rand::rngs::Xoshiro256PlusPlus;
use sparrow::consts::{DEFAULT_COMPRESS_TIME_RATIO, DEFAULT_EXPLORE_TIME_RATIO, DEFAULT_FAIL_DECAY_RATIO_CMPR, DEFAULT_MAX_CONSEQ_FAILS_EXPL, LOG_LEVEL_FILTER_DEBUG, LOG_LEVEL_FILTER_RELEASE};
use sparrow::util::ctrlc_terminator::CtrlCTerminator;
use sparrow::util::svg_exporter::SvgExporter;
use sparrow::util::terminator::Terminator;
use sparrow::util::listener::{ReportType, SolutionListener};

pub const OUTPUT_DIR: &str = "output";

pub const LIVE_DIR: &str = "data/live";

fn main() -> Result<()>{
    let mut config = DEFAULT_SPARROW_CONFIG;

    fs::create_dir_all(OUTPUT_DIR)?;
    let log_file_path = format!("{}/log.txt", OUTPUT_DIR);
    match cfg!(debug_assertions) {
        true => io::init_logger(LOG_LEVEL_FILTER_DEBUG, Path::new(&log_file_path))?,
        false => io::init_logger(LOG_LEVEL_FILTER_RELEASE, Path::new(&log_file_path))?,
    }

    let args = MainCli::parse();
    let input_file_path = &args.input;
    let (explore_dur, compress_dur) = match (args.global_time, args.exploration, args.compression) {
        (Some(gt), None, None) => {
            (Duration::from_secs(gt).mul_f32(DEFAULT_EXPLORE_TIME_RATIO), Duration::from_secs(gt).mul_f32(DEFAULT_COMPRESS_TIME_RATIO))
        },
        (None, Some(et), Some(ct)) => {
            (Duration::from_secs(et), Duration::from_secs(ct))
        },
        (None, None, None) => {
            warn!("[MAIN] no time limit specified");
            (Duration::from_secs(600).mul_f32(DEFAULT_EXPLORE_TIME_RATIO), Duration::from_secs(600).mul_f32(DEFAULT_COMPRESS_TIME_RATIO))
        },
        _ => bail!("invalid cli pattern (clap should have caught this)"),
    };
    config.expl_cfg.time_limit = explore_dur;
    config.cmpr_cfg.time_limit = compress_dur;
    let target_width:f32 = args.target_width.unwrap_or_else(|| 0.0);
    config.expl_cfg.target_width = target_width;
    config.cmpr_cfg.target_width = target_width;

    if args.early_termination {
        config.expl_cfg.max_conseq_failed_attempts = Some(DEFAULT_MAX_CONSEQ_FAILS_EXPL);
        config.cmpr_cfg.shrink_decay = ShrinkDecayStrategy::FailureBased(DEFAULT_FAIL_DECAY_RATIO_CMPR);
        warn!("[MAIN] early termination enabled!");
    }
    if let Some(arg_rng_seed) = args.rng_seed {
        config.rng_seed = Some(arg_rng_seed as usize);
    }

    info!("[MAIN] configured to explore for {}s and compress for {}s", explore_dur.as_secs(), compress_dur.as_secs());

    let mut rng = match config.rng_seed {
        Some(seed) => {
            info!("[MAIN] using seed: {}", seed);
            Xoshiro256PlusPlus::seed_from_u64(seed as u64)
        },
        None => {
            let seed = rand::random();
            warn!("[MAIN] no seed provided, using: {}", seed);
            Xoshiro256PlusPlus::seed_from_u64(seed)
        }
    };

    info!("[MAIN] system time: {}", jiff::Timestamp::now());

    let (ext_instance, ext_solution) = io::read_spp_input(Path::new(&input_file_path))?;

    let importer = Importer::new(config.cde_config, config.poly_simpl_tolerance, config.min_item_separation, config.narrow_concavity_cutoff_ratio);
    let instance = jagua_rs::probs::spp::io::import_instance(&importer, &ext_instance)?;

    let initial_solution:Option<SPSolution> = None;

    info!("[MAIN] loaded instance {} with #{} items", ext_instance.name, instance.total_item_qty());
    let mut start_index:usize =0;
    let mut total:usize =0;
    let mut end_index:usize = 1;
    let mut svg_exporter = {
        let final_svg_path = Some(format!("{OUTPUT_DIR}/final_{}_{1}_{2}.svg", ext_instance.name,start_index,end_index));

        let intermediate_svg_dir = match cfg!(feature = "only_final_svg") {
            true => None,
            false => Some(format!("{OUTPUT_DIR}/sols_{}", ext_instance.name))
        };

        let live_svg_path = match cfg!(feature = "live_svg") {
            true => Some(format!("{LIVE_DIR}/.live_solution.svg")),
            false => None
        };
        
        SvgExporter::new(
            final_svg_path,
            intermediate_svg_dir,
            live_svg_path
        )
    };
    
    let mut ctrlc_terminator = CtrlCTerminator::new();

    let name = ext_instance.name.clone();
    let mut finished:bool = false;

    let current_solution = initial_solution.clone();
    let mut best_solution:SPSolution = SPProblem::new(instance.clone()).save();

    while !finished && !ctrlc_terminator.kill() {
        info!("[MAIN] while loop iteration {} {1} : {2}",start_index,end_index,instance.total_item_qty());
        let current_ext_instance = ext_instance.clone();

        let solution = optimize(
            instance.clone(),
            rng.clone(),
            &mut svg_exporter,
            &mut ctrlc_terminator,
            &config.expl_cfg,
            &config.cmpr_cfg,
            current_solution.as_ref(),
            start_index,
            end_index
        );


        if solution.strip_width() <= target_width && end_index < instance.total_item_qty() {
            end_index+=1;
            end_index = std::cmp::min(instance.total_item_qty(), end_index);
           }
        else {
            if end_index == instance.total_item_qty() && solution.strip_width() < target_width {
                best_solution = solution.clone();
                finished = true;
                info!("[MAIN] finished successfully by end_index");
            }
            let indices: String = (start_index + 1).to_string() + "_" + &(end_index -1).to_string();
            let json_path = format!("{OUTPUT_DIR}/final_{0}{1}.json", name, indices);
            let json_output = ExtSPOutput {
                instance: current_ext_instance,
                solution: jagua_rs::probs::spp::io::export(&instance, &best_solution, *EPOCH)
            };
            io::write_json(&json_output, Path::new(json_path.as_str()), Level::Info)?;
            svg_exporter.final_path = Some(format!("{OUTPUT_DIR}/final_{}_{1}_{2}.svg", ext_instance.name,start_index+1,end_index-1));
            svg_exporter.report(ReportType::Final, &best_solution, &instance);
            total += end_index-1 - start_index;
            start_index = total;
            end_index = std::cmp::min(instance.total_item_qty(), total+1);
            info!("[MAIN] start_index is {} end_index is {1}", start_index,end_index);
            if start_index == end_index {
                info!("[MAIN] finished successfully by start_index == end_index");
                finished = true;

            }
        }
        ctrlc_terminator.timeout = None;
        best_solution = solution.clone();

    }
    info!("[MAIN] made it out of while loop. total panels placed is {}", total);
    Ok(())
}
