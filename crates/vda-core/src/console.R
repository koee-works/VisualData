  # A console beside an R step: the step's inputs stay loaded between runs.
  # Requests on stdin: "RUN <id> <bytes>" or "SAVE <id> <bytes>" and that
  # many bytes (code, or a path). Each reply is one line on stdout starting
  # with \x1eVDA; what the code prints is captured into the reply.
  json <- function(x) {
    if (is.null(x)) return("null")
    if (is.list(x)) {
      parts <- vapply(x, json, "")
      if (!is.null(names(x))) return(paste0("{", paste0(json_str(names(x)), ":", parts, collapse = ","), "}"))
      return(paste0("[", paste0(parts, collapse = ","), "]"))
    }
    if (length(x) != 1) return(json(as.list(x)))
    if (is.na(x)) return("null")
    if (is.logical(x)) return(if (x) "true" else "false")
    if (is.numeric(x)) return(if (is.finite(x)) format(x, digits = 15, scientific = FALSE, trim = TRUE) else json_str(as.character(x)))
    json_str(as.character(x))
  }
  json_str <- function(s) {
    s <- enc2utf8(as.character(s))
    s <- gsub("\\", "\\\\", s, fixed = TRUE)
    s <- gsub("\"", "\\\"", s, fixed = TRUE)
    s <- gsub("\n", "\\n", s, fixed = TRUE)
    s <- gsub("\r", "\\r", s, fixed = TRUE)
    s <- gsub("\t", "\\t", s, fixed = TRUE)
    s <- gsub("[[:cntrl:]]", " ", s)
    paste0("\"", s, "\"")
  }
  reply <- function(x) {
    cat("\036VDA ", json(x), "\n", sep = "", file = proto)
    flush(proto)
  }
  cell <- function(v) {
    if (is.na(v)) return(NULL)
    if (inherits(v, "Date")) return(format(v, "%Y-%m-%d"))
    if (inherits(v, "POSIXt")) return(format(v, "%Y-%m-%dT%H:%M:%OS6", tz = "UTC"))
    if (is.factor(v)) return(as.character(v))
    if (is.numeric(v) && !is.finite(v)) return(as.character(v))
    v
  }
  table <- function(d, limit = 200) {
    d <- as.data.frame(d, stringsAsFactors = FALSE, optional = TRUE)
    head <- d[seq_len(min(nrow(d), limit)), , drop = FALSE]
    rows <- lapply(seq_len(nrow(head)), function(i) lapply(seq_along(head), function(j) cell(head[[j]][[i]])))
    list(columns = lapply(seq_along(d), function(j) list(names(d)[j], class(d[[j]])[1])), rows = rows, total = nrow(d))
  }
  environment_list <- function(env) {
    out <- list()
    for (name in sort(ls(env))) {
      v <- get(name, envir = env)
      if (is.function(v)) next
      item <- if (is.data.frame(v)) {
        list(name = name, kind = "data frame", summary = sprintf("%s obs. of %d variables", format(nrow(v), big.mark = ","), ncol(v)))
      } else {
        s <- tryCatch(utils::capture.output(utils::str(v, give.attr = FALSE, vec.len = 3))[1], error = function(e) class(v)[1])
        list(name = name, kind = class(v)[1], summary = trimws(substr(s, 1, 80)))
      }
      out[[length(out) + 1]] <- item
    }
    out
  }

  proto <- stdout()
  args <- commandArgs(trailingOnly = TRUE)
  ids <- args
  env <- new.env(parent = globalenv())
  loaded <- tryCatch({
    dfs <- lapply(ids, read_input)
    source("params.R", local = TRUE)
    names(dfs) <- input_names
    env$dfs <- dfs
    env$df <- dfs[[1]]
    env$params <- params
    NULL
  }, error = function(e) conditionMessage(e))
  if (!is.null(loaded)) {
    reply(list(id = 0, ok = FALSE, error = list(message = paste("could not load the inputs:", loaded), line = NULL)))
    quit(save = "no", status = 1)
  }
  reply(list(id = 0, ok = TRUE, version = paste(R.version$major, R.version$minor, sep = "."), env = environment_list(env)))

  run <- function(rid, code) {
    st <- new.env()
    st$out <- character(); st$err <- character(); st$value <- NULL; st$error <- NULL
    exprs <- tryCatch(parse(text = code, keep.source = TRUE, encoding = "UTF-8"), error = function(e) {
      m <- conditionMessage(e)
      place <- regmatches(m, regexec(":([0-9]+):([0-9]+): ", m))[[1]]
      what <- sub("^[^:]*:[0-9]+:[0-9]+: ", "", strsplit(m, "\n")[[1]][1])
      st$error <- list(message = paste("syntax error:", what), line = if (length(place) == 3) as.integer(place[2]) else NULL)
      NULL
    })
    plots <- file.path(getwd(), sprintf("plot-%d-%%03d.png", rid))
    grDevices::png(plots, width = 760, height = 480, res = 110)
    refs <- attr(exprs, "srcref")
    for (i in seq_along(exprs)) {
      line <- if (!is.null(refs)) refs[[i]][1] else i
      failed <- tryCatch(withCallingHandlers({
        printed <- utils::capture.output(v <- withVisible(eval(exprs[[i]], env)))
        st$out <- c(st$out, printed)
        if (v$visible) {
          if (i == length(exprs) && is.data.frame(v$value)) st$value <- list(table = table(v$value))
          else st$out <- c(st$out, utils::capture.output(print(v$value)))
        }
        FALSE
      },
      packageStartupMessage = function(m) invokeRestart("muffleMessage"),
      message = function(m) { st$err <- c(st$err, sub("\n$", "", conditionMessage(m))); invokeRestart("muffleMessage") },
      warning = function(w) { st$err <- c(st$err, sprintf("Warning (line %d): %s", line, conditionMessage(w))); invokeRestart("muffleWarning") }),
      error = function(e) { st$error <- list(message = conditionMessage(e), line = line); TRUE },
      interrupt = function(e) { st$error <- list(message = "interrupted", line = line); TRUE })
      if (failed) break
    }
    grDevices::dev.off()
    images <- as.list(list.files(getwd(), pattern = sprintf("^plot-%d-[0-9]+[.]png$", rid), full.names = TRUE))
    reply(list(id = rid, ok = is.null(st$error), stdout = paste(st$out, collapse = "\n"), stderr = paste(st$err, collapse = "\n"),
               value = st$value, error = st$error, images = images, env = environment_list(env)))
  }

  con <- file("stdin", "rb")
  repeat {
    header <- tryCatch(readLines(con, n = 1, warn = FALSE), interrupt = function(e) character())
    if (!length(header)) break
    parts <- strsplit(header, " ", fixed = TRUE)[[1]]
    rid <- as.integer(parts[2])
    body <- rawToChar(readBin(con, "raw", as.integer(parts[3])))
    Encoding(body) <- "UTF-8"
    if (parts[1] == "RUN") {
      run(rid, body)
    } else if (parts[1] == "SAVE") {
      saved <- tryCatch({
        r <- get0("result", envir = env, inherits = FALSE)
        if (!is.data.frame(r)) stop("there is no data frame named `result` in the console")
        write_output(r, body)
        NULL
      }, error = function(e) conditionMessage(e))
      reply(list(id = rid, ok = is.null(saved), error = if (is.null(saved)) NULL else list(message = saved, line = NULL)))
    } else {
      break
    }
  }
