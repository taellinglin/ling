let wasm_bindgen = (function(exports) {
    let script_src;
    if (typeof document !== 'undefined' && document.currentScript !== null) {
        script_src = new URL(document.currentScript.src, location.href).toString();
    }

    /**
     * Initialise the WebGL2 context on the given OffscreenCanvas.
     * Must be called once before run_program.
     * @param {OffscreenCanvas} canvas
     */
    function init_canvas(canvas) {
        wasm.init_canvas(canvas);
    }
    exports.init_canvas = init_canvas;

    /**
     * Parse and execute a Ling source program.
     * Graphics output is rendered via the WebGL2 context set up by init_canvas.
     * @param {string} source
     */
    function run_program(source) {
        const ptr0 = passStringToWasm0(source, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        wasm.run_program(ptr0, len0);
    }
    exports.run_program = run_program;
    function __wbg_get_imports() {
        const import0 = {
            __proto__: null,
            __wbg___wbindgen_boolean_get_c3dd5c39f1b5a12b: function(arg0) {
                const v = arg0;
                const ret = typeof(v) === 'boolean' ? v : undefined;
                return isLikeNone(ret) ? 0xFFFFFF : ret ? 1 : 0;
            },
            __wbg___wbindgen_debug_string_07cb72cfcc952e2b: function(arg0, arg1) {
                const ret = debugString(arg1);
                const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
                const len1 = WASM_VECTOR_LEN;
                getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
                getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
            },
            __wbg___wbindgen_is_function_2f0fd7ceb86e64c5: function(arg0) {
                const ret = typeof(arg0) === 'function';
                return ret;
            },
            __wbg___wbindgen_throw_9c75d47bf9e7731e: function(arg0, arg1) {
                throw new Error(getStringFromWasm0(arg0, arg1));
            },
            __wbg_apply_0f21c8b7ff1b23f8: function() { return handleError(function (arg0, arg1, arg2) {
                const ret = arg0.apply(arg1, arg2);
                return ret;
            }, arguments); },
            __wbg_attachShader_18d37e6a1936237b: function(arg0, arg1, arg2) {
                arg0.attachShader(arg1, arg2);
            },
            __wbg_bindBuffer_a77c5c8cfa41f082: function(arg0, arg1, arg2) {
                arg0.bindBuffer(arg1 >>> 0, arg2);
            },
            __wbg_bufferData_d3f76b87295685cb: function(arg0, arg1, arg2, arg3) {
                arg0.bufferData(arg1 >>> 0, arg2, arg3 >>> 0);
            },
            __wbg_clearColor_576b764e2014fd68: function(arg0, arg1, arg2, arg3, arg4) {
                arg0.clearColor(arg1, arg2, arg3, arg4);
            },
            __wbg_clear_4ea2bcc891545cba: function(arg0, arg1) {
                arg0.clear(arg1 >>> 0);
            },
            __wbg_compileShader_50b61cd1b374d531: function(arg0, arg1) {
                arg0.compileShader(arg1);
            },
            __wbg_connect_537322130c96ad44: function() { return handleError(function (arg0, arg1) {
                arg0.connect(arg1);
            }, arguments); },
            __wbg_connect_b0c6d44e9984ca8e: function() { return handleError(function (arg0, arg1) {
                const ret = arg0.connect(arg1);
                return ret;
            }, arguments); },
            __wbg_createBuffer_68a72615fda09cc7: function(arg0) {
                const ret = arg0.createBuffer();
                return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
            },
            __wbg_createGain_33464d2fccb13fb8: function() { return handleError(function (arg0) {
                const ret = arg0.createGain();
                return ret;
            }, arguments); },
            __wbg_createOscillator_c64f08349366ee0d: function() { return handleError(function (arg0) {
                const ret = arg0.createOscillator();
                return ret;
            }, arguments); },
            __wbg_createPanner_1368924886087f8f: function() { return handleError(function (arg0) {
                const ret = arg0.createPanner();
                return ret;
            }, arguments); },
            __wbg_createProgram_932959b0abef3889: function(arg0) {
                const ret = arg0.createProgram();
                return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
            },
            __wbg_createShader_195b98e391086cfb: function(arg0, arg1) {
                const ret = arg0.createShader(arg1 >>> 0);
                return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
            },
            __wbg_destination_a7fb84721246ff2f: function(arg0) {
                const ret = arg0.destination;
                return ret;
            },
            __wbg_disable_b0f20ab1b990a65d: function(arg0, arg1) {
                arg0.disable(arg1 >>> 0);
            },
            __wbg_drawArrays_d5a5cd7c06a36bac: function(arg0, arg1, arg2, arg3) {
                arg0.drawArrays(arg1 >>> 0, arg2, arg3);
            },
            __wbg_enableVertexAttribArray_16defb159a05d60a: function(arg0, arg1) {
                arg0.enableVertexAttribArray(arg1 >>> 0);
            },
            __wbg_error_48655ee7e4756f8b: function(arg0) {
                console.error(arg0);
            },
            __wbg_error_a6fa202b58aa1cd3: function(arg0, arg1) {
                let deferred0_0;
                let deferred0_1;
                try {
                    deferred0_0 = arg0;
                    deferred0_1 = arg1;
                    console.error(getStringFromWasm0(arg0, arg1));
                } finally {
                    wasm.__wbindgen_free(deferred0_0, deferred0_1, 1);
                }
            },
            __wbg_frequency_0f0dc1e8480ee4d1: function(arg0) {
                const ret = arg0.frequency;
                return ret;
            },
            __wbg_gain_c994bc21cdd2e1b9: function(arg0) {
                const ret = arg0.gain;
                return ret;
            },
            __wbg_getAttribLocation_cb58ec5962a16f41: function(arg0, arg1, arg2, arg3) {
                const ret = arg0.getAttribLocation(arg1, getStringFromWasm0(arg2, arg3));
                return ret;
            },
            __wbg_getContext_5d4707454276e47f: function() { return handleError(function (arg0, arg1, arg2) {
                const ret = arg0.getContext(getStringFromWasm0(arg1, arg2));
                return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
            }, arguments); },
            __wbg_getProgramInfoLog_f93553deba23cccc: function(arg0, arg1, arg2) {
                const ret = arg1.getProgramInfoLog(arg2);
                var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
                var len1 = WASM_VECTOR_LEN;
                getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
                getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
            },
            __wbg_getProgramParameter_a00a3869258b814e: function(arg0, arg1, arg2) {
                const ret = arg0.getProgramParameter(arg1, arg2 >>> 0);
                return ret;
            },
            __wbg_getShaderInfoLog_25f08216f6d590f6: function(arg0, arg1, arg2) {
                const ret = arg1.getShaderInfoLog(arg2);
                var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
                var len1 = WASM_VECTOR_LEN;
                getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
                getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
            },
            __wbg_getShaderParameter_96635c982831e95b: function(arg0, arg1, arg2) {
                const ret = arg0.getShaderParameter(arg1, arg2 >>> 0);
                return ret;
            },
            __wbg_getUniformLocation_484ff1965b8e30f4: function(arg0, arg1, arg2, arg3) {
                const ret = arg0.getUniformLocation(arg1, getStringFromWasm0(arg2, arg3));
                return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
            },
            __wbg_get_41476db20fef99a8: function() { return handleError(function (arg0, arg1) {
                const ret = Reflect.get(arg0, arg1);
                return ret;
            }, arguments); },
            __wbg_height_900decaf28c42054: function(arg0) {
                const ret = arg0.height;
                return ret;
            },
            __wbg_instanceof_WebGl2RenderingContext_fbfd73b8b9465e2d: function(arg0) {
                let result;
                try {
                    result = arg0 instanceof WebGL2RenderingContext;
                } catch (_) {
                    result = false;
                }
                const ret = result;
                return ret;
            },
            __wbg_linkProgram_76940d17b54d375b: function(arg0, arg1) {
                arg0.linkProgram(arg1);
            },
            __wbg_listener_fdeaed80e0133abf: function(arg0) {
                const ret = arg0.listener;
                return ret;
            },
            __wbg_new_227d7c05414eb861: function() {
                const ret = new Error();
                return ret;
            },
            __wbg_new_3baa8d9866155c79: function() {
                const ret = new Array();
                return ret;
            },
            __wbg_new_a6b46eaf9085fbeb: function() { return handleError(function () {
                const ret = new lAudioContext();
                return ret;
            }, arguments); },
            __wbg_push_60a5366c0bb22a7d: function(arg0, arg1) {
                const ret = arg0.push(arg1);
                return ret;
            },
            __wbg_set_height_77937c921db92223: function(arg0, arg1) {
                arg0.height = arg1 >>> 0;
            },
            __wbg_set_type_1425359fd8e9769c: function(arg0, arg1) {
                arg0.type = __wbindgen_enum_OscillatorType[arg1];
            },
            __wbg_set_value_78631e9dc5b69626: function(arg0, arg1) {
                arg0.value = arg1;
            },
            __wbg_set_width_da52058a27694474: function(arg0, arg1) {
                arg0.width = arg1 >>> 0;
            },
            __wbg_shaderSource_0aa654ee0e007aa6: function(arg0, arg1, arg2, arg3) {
                arg0.shaderSource(arg1, getStringFromWasm0(arg2, arg3));
            },
            __wbg_stack_3b0d974bbf31e44f: function(arg0, arg1) {
                const ret = arg1.stack;
                const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
                const len1 = WASM_VECTOR_LEN;
                getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
                getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
            },
            __wbg_start_bd3a632033118000: function() { return handleError(function (arg0) {
                arg0.start();
            }, arguments); },
            __wbg_uniform2f_271a3fd2bb77ca1e: function(arg0, arg1, arg2, arg3) {
                arg0.uniform2f(arg1, arg2, arg3);
            },
            __wbg_useProgram_330a8a331113dc40: function(arg0, arg1) {
                arg0.useProgram(arg1);
            },
            __wbg_vertexAttribPointer_734b53a3b8f492ca: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
                arg0.vertexAttribPointer(arg1 >>> 0, arg2, arg3 >>> 0, arg4 !== 0, arg5, arg6);
            },
            __wbg_viewport_d56ad9cd4b4e71ca: function(arg0, arg1, arg2, arg3, arg4) {
                arg0.viewport(arg1, arg2, arg3, arg4);
            },
            __wbg_warn_1f9b94806da61fbb: function(arg0) {
                console.warn(arg0);
            },
            __wbg_width_bb0a84dddb1bba27: function(arg0) {
                const ret = arg0.width;
                return ret;
            },
            __wbindgen_cast_0000000000000001: function(arg0) {
                // Cast intrinsic for `F64 -> Externref`.
                const ret = arg0;
                return ret;
            },
            __wbindgen_cast_0000000000000002: function(arg0, arg1) {
                // Cast intrinsic for `Ref(Slice(F32)) -> NamedExternref("Float32Array")`.
                const ret = getArrayF32FromWasm0(arg0, arg1);
                return ret;
            },
            __wbindgen_cast_0000000000000003: function(arg0, arg1) {
                // Cast intrinsic for `Ref(String) -> Externref`.
                const ret = getStringFromWasm0(arg0, arg1);
                return ret;
            },
            __wbindgen_init_externref_table: function() {
                const table = wasm.__wbindgen_externrefs;
                const offset = table.grow(4);
                table.set(0, undefined);
                table.set(offset + 0, undefined);
                table.set(offset + 1, null);
                table.set(offset + 2, true);
                table.set(offset + 3, false);
            },
        };
        return {
            __proto__: null,
            "./ling_wasm_bg.js": import0,
        };
    }

    const lAudioContext = (typeof AudioContext !== 'undefined' ? AudioContext : (typeof webkitAudioContext !== 'undefined' ? webkitAudioContext : undefined));
    const __wbindgen_enum_OscillatorType = ["sine", "square", "sawtooth", "triangle", "custom"];

    function addToExternrefTable0(obj) {
        const idx = wasm.__externref_table_alloc();
        wasm.__wbindgen_externrefs.set(idx, obj);
        return idx;
    }

    function debugString(val) {
        // primitive types
        const type = typeof val;
        if (type == 'number' || type == 'boolean' || val == null) {
            return  `${val}`;
        }
        if (type == 'string') {
            return `"${val}"`;
        }
        if (type == 'symbol') {
            const description = val.description;
            if (description == null) {
                return 'Symbol';
            } else {
                return `Symbol(${description})`;
            }
        }
        if (type == 'function') {
            const name = val.name;
            if (typeof name == 'string' && name.length > 0) {
                return `Function(${name})`;
            } else {
                return 'Function';
            }
        }
        // objects
        if (Array.isArray(val)) {
            const length = val.length;
            let debug = '[';
            if (length > 0) {
                debug += debugString(val[0]);
            }
            for(let i = 1; i < length; i++) {
                debug += ', ' + debugString(val[i]);
            }
            debug += ']';
            return debug;
        }
        // Test for built-in
        const builtInMatches = /\[object ([^\]]+)\]/.exec(toString.call(val));
        let className;
        if (builtInMatches && builtInMatches.length > 1) {
            className = builtInMatches[1];
        } else {
            // Failed to match the standard '[object ClassName]'
            return toString.call(val);
        }
        if (className == 'Object') {
            // we're a user defined class or Object
            // JSON.stringify avoids problems with cycles, and is generally much
            // easier than looping through ownProperties of `val`.
            try {
                return 'Object(' + JSON.stringify(val) + ')';
            } catch (_) {
                return 'Object';
            }
        }
        // errors
        if (val instanceof Error) {
            return `${val.name}: ${val.message}\n${val.stack}`;
        }
        // TODO we could test for more things here, like `Set`s and `Map`s.
        return className;
    }

    function getArrayF32FromWasm0(ptr, len) {
        ptr = ptr >>> 0;
        return getFloat32ArrayMemory0().subarray(ptr / 4, ptr / 4 + len);
    }

    let cachedDataViewMemory0 = null;
    function getDataViewMemory0() {
        if (cachedDataViewMemory0 === null || cachedDataViewMemory0.buffer.detached === true || (cachedDataViewMemory0.buffer.detached === undefined && cachedDataViewMemory0.buffer !== wasm.memory.buffer)) {
            cachedDataViewMemory0 = new DataView(wasm.memory.buffer);
        }
        return cachedDataViewMemory0;
    }

    let cachedFloat32ArrayMemory0 = null;
    function getFloat32ArrayMemory0() {
        if (cachedFloat32ArrayMemory0 === null || cachedFloat32ArrayMemory0.byteLength === 0) {
            cachedFloat32ArrayMemory0 = new Float32Array(wasm.memory.buffer);
        }
        return cachedFloat32ArrayMemory0;
    }

    function getStringFromWasm0(ptr, len) {
        return decodeText(ptr >>> 0, len);
    }

    let cachedUint8ArrayMemory0 = null;
    function getUint8ArrayMemory0() {
        if (cachedUint8ArrayMemory0 === null || cachedUint8ArrayMemory0.byteLength === 0) {
            cachedUint8ArrayMemory0 = new Uint8Array(wasm.memory.buffer);
        }
        return cachedUint8ArrayMemory0;
    }

    function handleError(f, args) {
        try {
            return f.apply(this, args);
        } catch (e) {
            const idx = addToExternrefTable0(e);
            wasm.__wbindgen_exn_store(idx);
        }
    }

    function isLikeNone(x) {
        return x === undefined || x === null;
    }

    function passStringToWasm0(arg, malloc, realloc) {
        if (realloc === undefined) {
            const buf = cachedTextEncoder.encode(arg);
            const ptr = malloc(buf.length, 1) >>> 0;
            getUint8ArrayMemory0().subarray(ptr, ptr + buf.length).set(buf);
            WASM_VECTOR_LEN = buf.length;
            return ptr;
        }

        let len = arg.length;
        let ptr = malloc(len, 1) >>> 0;

        const mem = getUint8ArrayMemory0();

        let offset = 0;

        for (; offset < len; offset++) {
            const code = arg.charCodeAt(offset);
            if (code > 0x7F) break;
            mem[ptr + offset] = code;
        }
        if (offset !== len) {
            if (offset !== 0) {
                arg = arg.slice(offset);
            }
            ptr = realloc(ptr, len, len = offset + arg.length * 3, 1) >>> 0;
            const view = getUint8ArrayMemory0().subarray(ptr + offset, ptr + len);
            const ret = cachedTextEncoder.encodeInto(arg, view);

            offset += ret.written;
            ptr = realloc(ptr, len, offset, 1) >>> 0;
        }

        WASM_VECTOR_LEN = offset;
        return ptr;
    }

    let cachedTextDecoder = new TextDecoder('utf-8', { ignoreBOM: true, fatal: true });
    cachedTextDecoder.decode();
    function decodeText(ptr, len) {
        return cachedTextDecoder.decode(getUint8ArrayMemory0().subarray(ptr, ptr + len));
    }

    const cachedTextEncoder = new TextEncoder();

    if (!('encodeInto' in cachedTextEncoder)) {
        cachedTextEncoder.encodeInto = function (arg, view) {
            const buf = cachedTextEncoder.encode(arg);
            view.set(buf);
            return {
                read: arg.length,
                written: buf.length
            };
        };
    }

    let WASM_VECTOR_LEN = 0;

    let wasmModule, wasmInstance, wasm;
    function __wbg_finalize_init(instance, module) {
        wasmInstance = instance;
        wasm = instance.exports;
        wasmModule = module;
        cachedDataViewMemory0 = null;
        cachedFloat32ArrayMemory0 = null;
        cachedUint8ArrayMemory0 = null;
        wasm.__wbindgen_start();
        return wasm;
    }

    async function __wbg_load(module, imports) {
        if (typeof Response === 'function' && module instanceof Response) {
            if (typeof WebAssembly.instantiateStreaming === 'function') {
                try {
                    return await WebAssembly.instantiateStreaming(module, imports);
                } catch (e) {
                    const validResponse = module.ok && expectedResponseType(module.type);

                    if (validResponse && module.headers.get('Content-Type') !== 'application/wasm') {
                        console.warn("`WebAssembly.instantiateStreaming` failed because your server does not serve Wasm with `application/wasm` MIME type. Falling back to `WebAssembly.instantiate` which is slower. Original error:\n", e);

                    } else { throw e; }
                }
            }

            const bytes = await module.arrayBuffer();
            return await WebAssembly.instantiate(bytes, imports);
        } else {
            const instance = await WebAssembly.instantiate(module, imports);

            if (instance instanceof WebAssembly.Instance) {
                return { instance, module };
            } else {
                return instance;
            }
        }

        function expectedResponseType(type) {
            switch (type) {
                case 'basic': case 'cors': case 'default': return true;
            }
            return false;
        }
    }

    function initSync(module) {
        if (wasm !== undefined) return wasm;


        if (module !== undefined) {
            if (Object.getPrototypeOf(module) === Object.prototype) {
                ({module} = module)
            } else {
                console.warn('using deprecated parameters for `initSync()`; pass a single object instead')
            }
        }

        const imports = __wbg_get_imports();
        if (!(module instanceof WebAssembly.Module)) {
            module = new WebAssembly.Module(module);
        }
        const instance = new WebAssembly.Instance(module, imports);
        return __wbg_finalize_init(instance, module);
    }

    async function __wbg_init(module_or_path) {
        if (wasm !== undefined) return wasm;


        if (module_or_path !== undefined) {
            if (Object.getPrototypeOf(module_or_path) === Object.prototype) {
                ({module_or_path} = module_or_path)
            } else {
                console.warn('using deprecated parameters for the initialization function; pass a single object instead')
            }
        }

        if (module_or_path === undefined && script_src !== undefined) {
            module_or_path = script_src.replace(/\.js$/, "_bg.wasm");
        }
        const imports = __wbg_get_imports();

        if (typeof module_or_path === 'string' || (typeof Request === 'function' && module_or_path instanceof Request) || (typeof URL === 'function' && module_or_path instanceof URL)) {
            module_or_path = fetch(module_or_path);
        }

        const { instance, module } = await __wbg_load(await module_or_path, imports);

        return __wbg_finalize_init(instance, module);
    }

    return Object.assign(__wbg_init, { initSync }, exports);
})({ __proto__: null });
