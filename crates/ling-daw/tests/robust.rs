//! Hostile-input robustness: no `daw_*` call may panic (a panic aborts the whole IDE in release).
use ling_daw::{Daw, Val};

const NAMES: &str = "daw_clip_file daw_smp_zones daw_smp_fx daw_smp_names daw_smp_add daw_smp_remove daw_smp_set daw_smp_detect daw_smp_autoloop daw_smp_spread daw_smp_peaks daw_smp_env daw_smp_drop_env daw_smp_find daw_smp_pick daw_smp_roots_set daw_smp_forget daw_kar_scan daw_kar_lines daw_kar_ffmpeg daw_kar_font_family daw_kar_write daw_kar_video daw_kar_video_state daw_kar_video_message daw_kar_video_ack daw_sheet_scan daw_sheet_png daw_sheet_geo daw_sheet_edit daw_sheet_xml daw_sheet_musescore_find daw_sheet_musescore_open daw_sheet_write daw_trk_channels daw_trk_length daw_trk_page daw_trk_ring daw_trk_set daw_rig_view daw_rig_catalog daw_rig_presets daw_inst_categories daw_mix_start daw_mix_poll daw_mix_cancel daw_mix_tracks daw_mix_master daw_mix_set_role daw_mix_style daw_mix_plan daw_mix_apply daw_mix_prompt daw_mix_system daw_mix_parse daw_script_check daw_script_fx_add daw_script_inst_add daw_script_reload daw_fx_command daw_fx_info daw_clip_set_notes daw_vm_scales daw_vm_temperaments daw_vm_convert daw_vm_info daw_vm_set daw_vm_snap daw_vm_quantize daw_vm_render daw_arr_glue_scan daw_arr_glue_apply daw_trk_pdf daw_duck_keys daw_duck_add daw_duck_list daw_duck_remove daw_duck_set daw_duck_get daw_duck_shape daw_key_get daw_key_set daw_key_detect daw_key_tracks daw_key_apply daw_lyrf_prepare daw_lyrf_system daw_lyrf_prompt daw_lyrf_accept daw_lyrf_info daw_lyrf_apply daw_auto_lane_empty daw_auto_points_range daw_derive_example daw_artic_begin daw_artic_prompt daw_artic_system daw_artic_apply daw_artic_info daw_artic_end daw_inst_random daw_fx_load daw_fx_source daw_arr_slice_scan daw_arr_slice_apply daw_arr_name daw_fx_view daw_fx_presets daw_fx_preset daw_fx_reset daw_fx_random daw_th_harm_summary daw_th_harmonize daw_notes_apply daw_inst_default daw_fx_default daw_auto_shape daw_arr_struct_scan daw_arr_struct_apply daw_ai_genre_prompt daw_ai_genres daw_ai_parse_genre daw_ai_guess_genre daw_vox_sources daw_vox_tune daw_fix_scan daw_fix_run daw_fix_report daw_arr_scan daw_arr_fix daw_arr_sections daw_arr_tags_set daw_arr_tags daw_roll_arp daw_derive_prompt daw_derive_apply daw_derive_local daw_th_summary daw_th_keys daw_th_chords daw_th_roles daw_th_phrases daw_th_next daw_th_insert_chord daw_th_autocorrect daw_th_retune daw_th_vocab daw_th_spec daw_th_spec_summary daw_th_generate daw_style_thumbprint daw_style_fields daw_style_from_file daw_style_edit daw_lib_scan daw_ai_available daw_ai_example daw_ai_roles daw_ai_prompt daw_ai_choose daw_ai_style daw_inst_get_many daw_track_set_kind daw_midi_scan daw_inst_get daw_inst_plot daw_lyric_ipa daw_lyric_apply daw_lyric_text daw_note_lyrics daw_note_lyric_set daw_lyric_script daw_audio_rec_wave daw_audio_rec_info daw_audio_gain_get daw_audio_gain_set daw_audio_norm_get daw_audio_norm_set daw_audio_inputs daw_audio_rec_active daw_audio_rec_level daw_auto_lane_add daw_auto_lane_remove daw_auto_lanes daw_auto_points daw_auto_rec daw_auto_set_points daw_bpm daw_can_redo daw_can_undo daw_clip_add_midi daw_clip_count daw_clip_detect_bpm daw_clip_dup daw_clip_get daw_clip_name daw_clip_notes daw_clip_peaks daw_clip_remove daw_clip_set daw_clip_set_name daw_clip_split daw_clip_to_track daw_device daw_devices daw_dirty daw_end_beat daw_fx_add daw_fx_bypass daw_fx_bypassed daw_fx_count daw_fx_move daw_fx_name daw_fx_params daw_fx_remove daw_fx_set daw_fx_types daw_inst_params daw_inst_part daw_inst_parts daw_inst_preset daw_inst_presets daw_inst_select_part daw_inst_set daw_live daw_loop_end daw_loop_on daw_loop_start daw_master_get daw_master_set daw_meta_get daw_meta_set daw_midi_current daw_midi_inputs daw_midi_target daw_note_add daw_note_remove daw_note_set daw_palette daw_pause daw_perf_auto daw_perf_launch daw_perf_launch_scene daw_perf_only daw_perf_quant daw_perf_rec daw_perf_scene_add daw_perf_scene_name daw_perf_scene_remove daw_perf_scene_set_name daw_perf_scenes daw_perf_set_only daw_perf_set_quant daw_perf_slot daw_perf_slot_clear daw_perf_slot_copy daw_perf_slot_from_clip daw_perf_slot_info daw_perf_slot_new daw_perf_state daw_perf_stop_all daw_perf_stop_track daw_play daw_playing daw_position daw_project_name daw_project_path daw_rec_arm daw_redo daw_route_add daw_route_count daw_route_get daw_route_remove daw_route_set daw_sample_rate daw_seek daw_set_bpm daw_set_dirty daw_set_loop daw_set_project_name daw_set_project_path daw_status daw_stop daw_track_count daw_track_dup daw_track_get daw_track_inst daw_track_kind daw_track_move daw_track_name daw_track_remove daw_track_set daw_track_set_name daw_undo daw_undo_push daw_track_add daw_imp_version daw_imp_perf_styles daw_imp_master_preset daw_imp_mix_style daw_imp_produce daw_imp_sounds daw_imp_catalog daw_imp_choice daw_imp_kit daw_imp_pick";

fn n(x: f64) -> Val {
    Val::Num(x)
}
fn s(x: &str) -> Val {
    Val::Str(x.into())
}

fn hostile_args() -> Vec<Vec<Val>> {
    let weird = [0.0, 1.0, 2.0, -1.0, 1e18, -1e18, f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 9.9e9, 4294967296.0];
    let mut v: Vec<Vec<Val>> = vec![vec![]];
    for a in weird {
        v.push(vec![n(a)]);
        v.push(vec![n(a), n(a)]);
        v.push(vec![n(a), n(0.0), n(a)]);
        v.push(vec![n(0.0), n(a), n(a), n(a)]);
        v.push(vec![n(1.0), n(a), n(0.5), n(a), n(a)]);
        v.push(vec![n(0.0), s("vol"), n(a)]);
        v.push(vec![n(0.0), n(0.0), s("start"), n(a)]);
        v.push(vec![n(0.0), n(0.0), n(0.0), s("len"), n(a)]);
    }
    v.push(vec![s("nan"), s("inf"), s("-inf")]);
    v.push(vec![s(""), s(""), s("")]);
    v.push(vec![n(0.0), n(0.0), s("[so-fa]-[rei][-][ ]-[\u{ac00}\u{e01}\u{3042}\u{4f60}"), s("auto")]);
    v.push(vec![s("[[[]]]---~~~"), s("korean")]);
    v.push(vec![n(0.0), Val::List(vec![n(f64::NAN), n(1.0), n(0.0), n(f64::NAN), n(2.0)])]);
    v.push(vec![n(0.0), n(0.0), Val::List(vec![n(f64::NAN), n(1.0), n(3.0), n(f64::INFINITY), n(1.0), n(0.5)])]);
    v
}

#[test]
fn no_daw_call_panics_on_hostile_input() {
    let mut d = Daw::new();
    let setup = |d: &mut Daw| {
        d.call("daw_new", &[]);
        for k in ["synth", "drums", "audio", "growl", "voice", "prism"] {
            d.call("daw_track_add", &[s(k), s("")]);
        }
        d.call("daw_clip_add_midi", &[n(0.0), n(0.0), n(4.0)]);
        d.call("daw_note_add", &[n(0.0), n(0.0), n(0.0), n(1.0), n(60.0), n(0.9)]);
        d.call("daw_perf_auto", &[n(1.0)]);
        d.call("daw_route_add", &[n(0.0), n(0.0), n(1.0)]);
        d.call("daw_fx_add", &[n(0.0), s("Compressor")]);
        d.call("daw_auto_lane_add", &[n(0.0), s("vol")]);
    };
    setup(&mut d);
    let args = hostile_args();
    for name in NAMES.split(' ') {
        for a in &args {
            let r = d.call(name, a);
            assert!(r.is_some(), "{name} not recognised");
        }
        // keep the project usable between functions
        d.call("daw_set_bpm", &[n(120.0)]);
        if d.call("daw_track_count", &[]).map(|r| r.unwrap() == n(0.0)).unwrap_or(false) {
            setup(&mut d);
        }
    }
    // the audio path must survive whatever state that left behind
    d.call("daw_set_bpm", &[n(120.0)]);
    d.call("daw_play", &[]);
    let eng = d.engine();
    let mut buf = vec![0.0f32; 48000 * 2];
    {
        let mut e = eng.lock().unwrap();
        for ch in buf.chunks_mut(1024) {
            e.render(ch);
        }
    }
    assert!(buf.iter().all(|x| x.is_finite()), "non-finite audio after hostile calls");
}

#[test]
fn track_ops_keep_engine_state_consistent() {
    let mut d = Daw::new();
    d.call("daw_new", &[]);
    for _ in 0..4 {
        d.call("daw_track_add", &[s("synth"), s("")]);
    }
    d.call("daw_clip_add_midi", &[n(1.0), n(0.0), n(4.0)]);
    d.call("daw_note_add", &[n(1.0), n(0.0), n(0.0), n(1.0), n(60.0), n(0.9)]);
    d.call("daw_perf_slot_from_clip", &[n(0.0), n(1.0), n(0.0)]);
    d.call("daw_perf_launch", &[n(0.0), n(1.0)]);
    d.call("daw_play", &[]);
    d.call("daw_track_remove", &[n(0.0)]);
    d.call("daw_track_move", &[n(2.0), n(-2.0)]);
    d.call("daw_track_remove", &[n(2.0)]);
    d.call("daw_track_remove", &[n(1.0)]);
    d.call("daw_track_remove", &[n(0.0)]);
    d.call("daw_track_remove", &[n(0.0)]); // now empty
    let eng = d.engine();
    let mut buf = vec![0.0f32; 4096];
    let mut e = eng.lock().unwrap();
    for ch in buf.chunks_mut(512) {
        e.render(ch);
    }
    assert_eq!(e.project.tracks.len(), 0);
}
